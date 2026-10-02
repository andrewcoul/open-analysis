//! Dividing frames where nodes lie on their spans. A node drawn on a beam
//! does not connect to it until the beam is split there: the solver only
//! joins elements that share a node. Splitting keeps the original frame as
//! the first piece, so its id, name, and every reference to it stay put,
//! and adds the rest as new frames that copy its properties, its group
//! memberships, and its share of every member load.
use crate::{command::*, entity::*, model::*};
use oa_core::units::{Length, LineLoad};
use std::collections::BTreeSet;

/// A node closer than this to a frame's axis, in metres, lies on it, and
/// one closer than this to either end is that end rather than a point on
/// the span.
pub const ON_SPAN_TOLERANCE: f64 = 1e-4;

/// The nodes strictly between a frame's ends that lie on its axis, with
/// their distance from the I end in metres, nearest first. `candidates`
/// limits which nodes are considered; None considers every node.
pub fn nodes_on_span(
    model: &Model,
    frame: EntityId,
    candidates: Option<&BTreeSet<EntityId>>,
) -> Vec<(EntityId, f64)> {
    let Some(f) = model.frames.get(&frame) else {
        return vec![];
    };
    let (Some(i), Some(j)) = (model.nodes.get(&f.nodes[0]), model.nodes.get(&f.nodes[1])) else {
        return vec![];
    };
    let a = i.position.map(|v| v.si());
    let b = j.position.map(|v| v.si());
    let axis: [f64; 3] = std::array::from_fn(|k| b[k] - a[k]);
    let length = axis.iter().map(|v| v * v).sum::<f64>().sqrt();
    if !length.is_finite() || length <= ON_SPAN_TOLERANCE {
        return vec![];
    }
    let mut out: Vec<(EntityId, f64)> = model
        .nodes
        .iter()
        .filter(|(id, _)| **id != f.nodes[0] && **id != f.nodes[1])
        .filter(|(id, _)| candidates.is_none_or(|c| c.contains(id)))
        .filter_map(|(id, n)| {
            let p = n.position.map(|v| v.si());
            let d: [f64; 3] = std::array::from_fn(|k| p[k] - a[k]);
            let along = (0..3).map(|k| d[k] * axis[k]).sum::<f64>() / length;
            let off = (0..3)
                .map(|k| (d[k] - axis[k] * along / length).powi(2))
                .sum::<f64>()
                .sqrt();
            (along > ON_SPAN_TOLERANCE
                && along < length - ON_SPAN_TOLERANCE
                && off <= ON_SPAN_TOLERANCE)
                .then_some((*id, along))
        })
        .collect();
    out.sort_by(|x, y| x.1.total_cmp(&y.1));
    // Two coincident nodes on the span would make a zero-length piece; the
    // first one, by id, takes the station.
    out.dedup_by(|later, earlier| later.1 - earlier.1 <= ON_SPAN_TOLERANCE);
    out
}

/// The commands that split `frames` (every frame when empty) at the nodes
/// in `nodes` (every node when empty) lying on their spans. New frames take
/// ids from the model's next free id onward, in frame order and then along
/// each span. No commands means nothing needed splitting.
pub fn plan_split_frames(
    model: &Model,
    frames: &[EntityId],
    nodes: &[EntityId],
) -> Result<Vec<Command>> {
    for id in frames.iter().chain(nodes) {
        if model.kind_of(*id).is_none() {
            return Err(ModelError::NotFound(*id));
        }
    }
    for id in frames {
        if !model.frames.contains_key(id) {
            return Err(ModelError::WrongKind {
                entity: model.describe(*id),
                expected: EntityKind::Frame,
                actual: model.kind_of(*id).unwrap(),
            });
        }
    }
    for id in nodes {
        if !model.nodes.contains_key(id) {
            return Err(ModelError::WrongKind {
                entity: model.describe(*id),
                expected: EntityKind::Node,
                actual: model.kind_of(*id).unwrap(),
            });
        }
    }
    let targets: Vec<EntityId> = if frames.is_empty() {
        model.frames.keys().copied().collect()
    } else {
        frames
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    let candidates: Option<BTreeSet<EntityId>> =
        (!nodes.is_empty()).then(|| nodes.iter().copied().collect());

    let mut next = model.next_id;
    let mut taken: BTreeSet<String> = model.frames.values().map(|f| f.name.clone()).collect();
    let mut frame_commands = vec![];
    let mut groups = model.groups.clone();
    let mut cases = model.load_cases.clone();
    let mut groups_changed = BTreeSet::new();
    let mut cases_changed = BTreeSet::new();
    for id in targets {
        let stops = nodes_on_span(model, id, candidates.as_ref());
        if stops.is_empty() {
            continue;
        }
        let original = &model.frames[&id];
        let length = stations_length(model, original);
        // Piece k runs between joints[k] and joints[k + 1], at fractions
        // t[k] and t[k + 1] of the way from node I to node J.
        let mut t = vec![0.0];
        t.extend(stops.iter().map(|(_, s)| *s / length));
        t.push(1.0);
        // Member load positions run along the member between the ends its
        // joint offsets and cardinal point move it to. Moving both ends
        // keeps the moved line's points at the same fractions, so the
        // stations scale with its length.
        let moved = crate::compile::frame_length(model, id).unwrap_or(length);
        let stations: Vec<f64> = t.iter().map(|t| t * moved).collect();
        let joint = joint_offsets(model, original)?;
        let mut joints = vec![original.nodes[0]];
        joints.extend(stops.iter().map(|(n, _)| *n));
        joints.push(original.nodes[1]);
        let count = stations.len() - 1;
        let mut ids = vec![id];
        for _ in 1..count {
            ids.push(EntityId(next));
            next += 1;
        }
        for k in 0..count {
            let mut piece = original.clone();
            piece.nodes = [joints[k], joints[k + 1]];
            // The I end's releases stay at the I end, the J end's at the J
            // end; the new joints inside the span are continuous.
            // So do the end offsets, the lengths inside the joints at the
            // member's two ends.
            for r in 0..6 {
                if k > 0 {
                    piece.releases[r] = false;
                }
                if k + 1 < count {
                    piece.releases[6 + r] = false;
                }
            }
            if k > 0 {
                piece.offsets.end[0] = Length::ZERO;
            }
            if k + 1 < count {
                piece.offsets.end[1] = Length::ZERO;
            }
            // Joint offsets are interpolated to the new joints, so every
            // piece lies on the original's moved line.
            if let Some([di, dj]) = joint {
                piece.offsets.axes = Axes::Global;
                piece.offsets.joint = [t[k], t[k + 1]]
                    .map(|t| std::array::from_fn(|i| Length::from_si(di[i] + (dj[i] - di[i]) * t)));
            }
            if k == 0 {
                frame_commands.push(Command::UpdateFrame { id, frame: piece });
            } else {
                piece.name = (k + 1..)
                    .map(|n| format!("{}-{n}", original.name))
                    .find(|name| !taken.contains(name))
                    .expect("unbounded");
                taken.insert(piece.name.clone());
                frame_commands.push(Command::AddFrame {
                    id: ids[k],
                    frame: piece,
                });
            }
        }
        for (gid, group) in groups.iter_mut() {
            if group.members.contains(&id) {
                group.members.extend(&ids[1..]);
                groups_changed.insert(*gid);
            }
        }
        for (cid, case) in cases.iter_mut() {
            if !case.member.iter().any(|l| l.member() == id) {
                continue;
            }
            case.member = std::mem::take(&mut case.member)
                .into_iter()
                .flat_map(|l| {
                    if l.member() == id {
                        divide_load(&l, &stations, &ids)
                    } else {
                        vec![l]
                    }
                })
                .collect();
            cases_changed.insert(*cid);
        }
    }
    let mut commands = frame_commands;
    commands.extend(groups_changed.into_iter().map(|id| Command::UpdateGroup {
        id,
        group: groups.remove(&id).unwrap(),
    }));
    commands.extend(cases_changed.into_iter().map(|id| Command::UpdateLoadCase {
        id,
        load_case: cases.remove(&id).unwrap(),
    }));
    Ok(commands)
}

/// A frame's joint offsets in global axes, end I then J, in metres; None
/// when it has none. The cardinal point is left out: every piece keeps it,
/// and it shifts each one alike across the line they share.
fn joint_offsets(model: &Model, f: &Frame) -> Result<Option<[[f64; 3]; 2]>> {
    if f.offsets.joint_si().iter().flatten().all(|v| *v == 0.0) {
        return Ok(None);
    }
    let mut plain = f.clone();
    plain.cardinal_point = CardinalPoint::Centroid;
    let nodes = f.nodes.map(|n| model.nodes[&n].position.map(|v| v.si()));
    let ends = crate::compile::solver_frame(
        model,
        &plain,
        [oa_core::NodeId(0), oa_core::NodeId(1)],
        0,
        0,
    )
    .and_then(|frame| frame.ends(nodes))
    .map_err(|e| ModelError::Invalid(format!("cannot split frame {:?}: {e}", f.name)))?;
    Ok(Some(std::array::from_fn(|k| {
        std::array::from_fn(|i| ends[k][i] - nodes[k][i])
    })))
}

fn stations_length(model: &Model, f: &Frame) -> f64 {
    let (a, b) = (&model.nodes[&f.nodes[0]], &model.nodes[&f.nodes[1]]);
    (0..3)
        .map(|k| (b.position[k].si() - a.position[k].si()).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// A member load on the whole frame as loads on its pieces. A point load
/// goes to the piece it falls on, the earlier one when it sits on a joint;
/// a distributed load is cut at the joints, its intensity interpolated so
/// each piece carries the part of the original that lies on it.
fn divide_load(load: &MemberLoad, stations: &[f64], ids: &[EntityId]) -> Vec<MemberLoad> {
    let count = ids.len();
    match load {
        MemberLoad::Point {
            position,
            force,
            moment,
            axes,
            ..
        } => {
            let x = position.si();
            let k = (0..count)
                .find(|k| x <= stations[k + 1] + ON_SPAN_TOLERANCE)
                .unwrap_or(count - 1);
            vec![MemberLoad::Point {
                member: ids[k],
                position: Length::from_si(
                    (x - stations[k]).clamp(0.0, stations[k + 1] - stations[k]),
                ),
                force: *force,
                moment: *moment,
                axes: *axes,
            }]
        }
        MemberLoad::Distributed {
            start,
            end,
            start_load,
            end_load,
            axes,
            ..
        } => {
            let (a, b) = (start.si(), end.si());
            let at = |x: f64| -> [LineLoad; 3] {
                let t = if b > a { (x - a) / (b - a) } else { 0.0 };
                std::array::from_fn(|i| {
                    LineLoad::from_si(
                        start_load[i].si() + (end_load[i].si() - start_load[i].si()) * t,
                    )
                })
            };
            let mut out = vec![];
            for k in 0..count {
                let (s0, s1) = (stations[k], stations[k + 1]);
                let (lo, hi) = (a.max(s0), b.min(s1));
                // A load of no length stays whole on the piece it starts on.
                let degenerate = b - a <= ON_SPAN_TOLERANCE;
                if degenerate {
                    if a <= s1 + ON_SPAN_TOLERANCE || k + 1 == count {
                        out.push(MemberLoad::Distributed {
                            member: ids[k],
                            start: Length::from_si((a - s0).clamp(0.0, s1 - s0)),
                            end: Length::from_si((b - s0).clamp(0.0, s1 - s0)),
                            start_load: *start_load,
                            end_load: *end_load,
                            axes: *axes,
                        });
                        break;
                    }
                    continue;
                }
                if hi - lo <= ON_SPAN_TOLERANCE {
                    continue;
                }
                out.push(MemberLoad::Distributed {
                    member: ids[k],
                    start: Length::from_si(lo - s0),
                    end: Length::from_si(hi - s0),
                    start_load: at(lo),
                    end_load: at(hi),
                    axes: *axes,
                });
            }
            out
        }
    }
}
