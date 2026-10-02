//! Copying nodes, frames and shells as ETABS's Replicate does: in a line,
//! around a vertical axis, mirrored in a vertical plane, or onto other
//! levels. A copy keeps every property of what it copies and, when asked,
//! its loads. A copied node that lands on a node already there, or on one
//! an earlier copy made, is that node, so copies join up with the model and
//! with each other; a copied frame or shell that would sit on the same
//! nodes as one already there is left out.
use crate::split::ON_SPAN_TOLERANCE;
use crate::{command::*, entity::*, levels::TOLERANCE, model::*};
use oa_core::units::{Angle, Force, Length, LineLoad, Moment};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The most copies one replication makes.
pub const MAX_COPIES: u32 = 1000;

fn one() -> u32 {
    1
}

/// Where the copies go.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Replication {
    /// `count` copies, the k-th moved k times `offset`.
    Linear {
        offset: [Length; 3],
        #[serde(default = "one")]
        count: u32,
    },
    /// `count` copies around the vertical axis through `center` in plan,
    /// the k-th turned k times `angle`, counterclockwise seen from above.
    Radial {
        center: [Length; 2],
        angle: Angle,
        #[serde(default = "one")]
        count: u32,
    },
    /// One copy reflected in the vertical plane through the plan line from
    /// `start` to `end`.
    Mirror {
        start: [Length; 2],
        end: [Length; 2],
    },
    /// One copy on each level in `to`, moved up by its elevation less that
    /// of `from`.
    Levels { from: EntityId, to: Vec<EntityId> },
}

/// A rigid motion p -> L p + t, L orthogonal.
#[derive(Debug, Clone, Copy)]
struct Motion {
    linear: [[f64; 3]; 3],
    shift: [f64; 3],
}
impl Motion {
    fn translation(shift: [f64; 3]) -> Self {
        Self {
            linear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            shift,
        }
    }
    /// A turn of `angle` about the vertical axis through `center`.
    fn turn(center: [f64; 2], angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        let linear = [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]];
        let shift = [
            center[0] - (c * center[0] - s * center[1]),
            center[1] - (s * center[0] + c * center[1]),
            0.0,
        ];
        Self { linear, shift }
    }
    /// A reflection in the vertical plane through the plan line a-b.
    fn mirror(a: [f64; 2], b: [f64; 2]) -> Self {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy);
        // Unit normal to the plane, in plan.
        let n = [-dy / len, dx / len];
        let linear = [
            [1.0 - 2.0 * n[0] * n[0], -2.0 * n[0] * n[1], 0.0],
            [-2.0 * n[0] * n[1], 1.0 - 2.0 * n[1] * n[1], 0.0],
            [0.0, 0.0, 1.0],
        ];
        let d = 2.0 * (a[0] * n[0] + a[1] * n[1]);
        Self {
            linear,
            shift: [d * n[0], d * n[1], 0.0],
        }
    }
    fn vector(&self, v: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|i| (0..3).map(|j| self.linear[i][j] * v[j]).sum())
    }
    fn point(&self, p: [f64; 3]) -> [f64; 3] {
        let v = self.vector(p);
        std::array::from_fn(|i| v[i] + self.shift[i])
    }
    /// -1 for a reflection, 1 otherwise.
    fn det(&self) -> f64 {
        let m = &self.linear;
        let d = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        d.signum()
    }
    fn reflects(&self) -> bool {
        self.det() < 0.0
    }
    /// Whether it turns or reflects anything, rather than only moving it.
    fn reorients(&self) -> bool {
        (0..3).any(|i| {
            (0..3).any(|j| (self.linear[i][j] - if i == j { 1.0 } else { 0.0 }).abs() > 1e-12)
        })
    }
    /// A moment or other axial vector turns with L and flips in a mirror.
    fn axial(&self, v: [f64; 3]) -> [f64; 3] {
        self.vector(v).map(|x| x * self.det())
    }
}

/// What a replication adds, and the commands that add it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplicationPlan {
    pub commands: Vec<Command>,
    pub nodes: Vec<EntityId>,
    pub frames: Vec<EntityId>,
    pub shells: Vec<EntityId>,
    /// Frames and shells left out because one already sat on their nodes.
    pub skipped: usize,
}
impl ReplicationPlan {
    /// Every entity it adds.
    pub fn created(&self) -> Vec<EntityId> {
        self.nodes
            .iter()
            .chain(&self.frames)
            .chain(&self.shells)
            .copied()
            .collect()
    }
}

/// Nodes by position, so a copied node finds one already where it lands
/// without a search of every node.
struct NodeIndex {
    cells: HashMap<[i64; 3], Vec<(EntityId, [f64; 3])>>,
}
impl NodeIndex {
    fn cell(p: [f64; 3]) -> [i64; 3] {
        p.map(|v| (v / ON_SPAN_TOLERANCE).floor() as i64)
    }
    fn insert(&mut self, id: EntityId, p: [f64; 3]) {
        self.cells.entry(Self::cell(p)).or_default().push((id, p));
    }
    /// The node within the tolerance of `p`, the lowest id when several are.
    fn find(&self, p: [f64; 3]) -> Option<EntityId> {
        let c = Self::cell(p);
        let mut best: Option<EntityId> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = self.cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) else {
                        continue;
                    };
                    for (id, q) in list {
                        let d = (0..3).map(|k| (p[k] - q[k]).powi(2)).sum::<f64>().sqrt();
                        if d <= ON_SPAN_TOLERANCE && best.is_none_or(|b| *id < b) {
                            best = Some(*id);
                        }
                    }
                }
            }
        }
        best
    }
}

/// The motions that place each copy.
fn motions(model: &Model, replication: &Replication) -> Result<Vec<Motion>> {
    let check_count = |count: u32| {
        if count == 0 || count > MAX_COPIES {
            return Err(ModelError::Invalid(format!(
                "the number of copies must be from 1 to {MAX_COPIES}"
            )));
        }
        Ok(())
    };
    let finite = |v: &[f64]| v.iter().all(|x| x.is_finite());
    Ok(match replication {
        Replication::Linear { offset, count } => {
            check_count(*count)?;
            let d = offset.map(|v| v.si());
            if !finite(&d) {
                return Err(ModelError::Invalid(
                    "replication offset must be finite".into(),
                ));
            }
            if d.iter().map(|v| v * v).sum::<f64>().sqrt() <= ON_SPAN_TOLERANCE {
                return Err(ModelError::Invalid(
                    "a linear replication needs an offset".into(),
                ));
            }
            (1..=*count)
                .map(|k| Motion::translation(d.map(|v| v * k as f64)))
                .collect()
        }
        Replication::Radial {
            center,
            angle,
            count,
        } => {
            check_count(*count)?;
            let c = center.map(|v| v.si());
            if !finite(&c) || !angle.si().is_finite() {
                return Err(ModelError::Invalid(
                    "replication center and angle must be finite".into(),
                ));
            }
            let turn = angle.si().rem_euclid(std::f64::consts::TAU);
            if turn.min(std::f64::consts::TAU - turn) <= 1e-9 {
                return Err(ModelError::Invalid(
                    "a radial replication needs an angle that is not a whole turn".into(),
                ));
            }
            (1..=*count)
                .map(|k| Motion::turn(c, angle.si() * k as f64))
                .collect()
        }
        Replication::Mirror { start, end } => {
            let [a, b] = [start, end].map(|p| p.map(|v| v.si()));
            if !finite(&a) || !finite(&b) {
                return Err(ModelError::Invalid(
                    "mirror line coordinates must be finite".into(),
                ));
            }
            if (b[0] - a[0]).hypot(b[1] - a[1]) <= TOLERANCE {
                return Err(ModelError::Invalid(
                    "a mirror line needs two different points".into(),
                ));
            }
            vec![Motion::mirror(a, b)]
        }
        Replication::Levels { from, to } => {
            let level = |id: &EntityId| {
                model
                    .levels
                    .get(id)
                    .ok_or_else(|| match model.kind_of(*id) {
                        Some(actual) => ModelError::WrongKind {
                            entity: model.describe(*id),
                            expected: EntityKind::Level,
                            actual,
                        },
                        None => ModelError::NotFound(*id),
                    })
            };
            let base = level(from)?.elevation.si();
            if to.is_empty() {
                return Err(ModelError::Invalid(
                    "a replication onto levels needs at least one level".into(),
                ));
            }
            if to.len() > MAX_COPIES as usize {
                return Err(ModelError::Invalid(format!(
                    "the number of copies must be from 1 to {MAX_COPIES}"
                )));
            }
            let mut seen = BTreeSet::new();
            let mut out = vec![];
            for id in to {
                let dz = level(id)?.elevation.si() - base;
                if !seen.insert(*id) || dz.abs() <= TOLERANCE {
                    return Err(ModelError::Invalid(format!(
                        "{} is listed twice or is the level copied from",
                        model.describe(*id)
                    )));
                }
                out.push(Motion::translation([0.0, 0.0, dz]));
            }
            out
        }
    })
}

/// The level a copied node binds to: one at the elevation its own level
/// moves to, else the highest at or below the node, else the lowest.
fn level_for(model: &Model, source: &Node, dz: f64, z: f64) -> EntityId {
    let moved = model
        .levels
        .get(&source.level)
        .map(|l| l.elevation.si() + dz);
    let by_elevation = model.levels_by_elevation();
    if let Some(e) = moved
        && let Some(id) = by_elevation
            .iter()
            .find(|id| (model.levels[id].elevation.si() - e).abs() <= TOLERANCE)
    {
        return *id;
    }
    by_elevation
        .iter()
        .rev()
        .find(|id| model.levels[id].elevation.si() <= z + TOLERANCE)
        .or(by_elevation.first())
        .copied()
        .unwrap_or(source.level)
}

/// The first unused name made of `name` without its trailing digits and a
/// number: copies of "B12" are "B13", "B14", and so on.
fn next_name(name: &str, taken: &mut BTreeSet<String>) -> String {
    let stem = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let start = name[stem.len()..].parse::<u64>().map_or(2, |n| n + 1);
    let stem = if stem.is_empty() { name } else { stem };
    let found = (start..)
        .map(|n| format!("{stem}{n}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("unbounded");
    taken.insert(found.clone());
    found
}

fn length3(v: [f64; 3]) -> [Length; 3] {
    v.map(Length::from_si)
}

/// The commands that copy `entities`, a mix of nodes, frames and shells, as
/// `replication` says. A frame or shell brings its nodes. New ids run from
/// the model's next free id, copy by copy, each copy's nodes, then frames,
/// then shells, in id order. With `loads`, every load case gains the copies'
/// share of the loads on what they copy.
pub fn plan_replicate(
    model: &Model,
    entities: &[EntityId],
    replication: &Replication,
    loads: bool,
) -> Result<ReplicationPlan> {
    if entities.is_empty() {
        return Err(ModelError::Invalid("nothing to replicate".into()));
    }
    let mut nodes = BTreeSet::new();
    let mut frames = BTreeSet::new();
    let mut shells = BTreeSet::new();
    for id in entities {
        match model.kind_of(*id) {
            Some(EntityKind::Node) => {
                nodes.insert(*id);
            }
            Some(EntityKind::Frame) => {
                frames.insert(*id);
                nodes.extend(model.frames[id].nodes);
            }
            Some(EntityKind::Shell) => {
                shells.insert(*id);
                nodes.extend(model.shells[id].nodes);
            }
            Some(kind) => {
                return Err(ModelError::Invalid(format!(
                    "only nodes, frames and shells can be replicated, not a {kind} ({})",
                    model.describe(*id)
                )));
            }
            None => return Err(ModelError::NotFound(*id)),
        }
    }
    let motions = motions(model, replication)?;

    let mut index = NodeIndex {
        cells: HashMap::new(),
    };
    for (id, n) in &model.nodes {
        index.insert(*id, n.position.map(|v| v.si()));
    }
    // Frames and shells by their node sets, to leave out copies that would
    // double one already there.
    let mut frame_keys: BTreeSet<[EntityId; 2]> = model
        .frames
        .values()
        .map(|f| {
            let mut k = f.nodes;
            k.sort();
            k
        })
        .collect();
    let mut shell_keys: BTreeSet<[EntityId; 4]> = model
        .shells
        .values()
        .map(|s| {
            let mut k = s.nodes;
            k.sort();
            k
        })
        .collect();
    let mut node_names: BTreeSet<String> = model.nodes.values().map(|n| n.name.clone()).collect();
    let mut frame_names: BTreeSet<String> = model.frames.values().map(|f| f.name.clone()).collect();
    let mut shell_names: BTreeSet<String> = model.shells.values().map(|s| s.name.clone()).collect();

    // Each frame's local axes, joint offsets in global axes, and member
    // load direction, read once from the original.
    let mut frame_geometry = BTreeMap::new();
    let reorients = motions.iter().any(Motion::reorients);
    for id in frames.iter().filter(|_| reorients) {
        frame_geometry.insert(*id, frame_geometry_of(model, &model.frames[id])?);
    }

    let mut next = model.next_id;
    let mut plan = ReplicationPlan::default();
    let mut adds = vec![];
    let mut cases = model.load_cases.clone();
    let mut cases_changed = BTreeSet::new();
    for motion in &motions {
        // Where each source node lands: an existing node, or a new one.
        let mut map: BTreeMap<EntityId, EntityId> = BTreeMap::new();
        let mut made: BTreeSet<EntityId> = BTreeSet::new();
        for id in &nodes {
            let source = &model.nodes[id];
            let p = source.position.map(|v| v.si());
            let q = motion.point(p);
            if let Some(found) = index.find(q) {
                map.insert(*id, found);
                continue;
            }
            let new = EntityId(next);
            next += 1;
            let mut node = source.clone();
            node.name = next_name(&source.name, &mut node_names);
            node.position = length3(q);
            node.level = level_for(model, source, q[2] - p[2], q[2]);
            index.insert(new, q);
            map.insert(*id, new);
            made.insert(new);
            plan.nodes.push(new);
            adds.push(Command::AddNode { id: new, node });
        }
        let mut frame_map = BTreeMap::new();
        for id in &frames {
            let source = &model.frames[id];
            let ends = source.nodes.map(|n| map[&n]);
            let mut key = ends;
            key.sort();
            if !frame_keys.insert(key) {
                plan.skipped += 1;
                continue;
            }
            let new = EntityId(next);
            next += 1;
            let mut frame = source.clone();
            frame.name = next_name(&source.name, &mut frame_names);
            frame.nodes = ends;
            if motion.reorients() {
                let geometry = &frame_geometry[id];
                frame.local_y = Some(motion.vector(geometry.y));
                frame.roll = Angle::ZERO;
                if let Some(joint) = geometry.joint {
                    frame.offsets.axes = Axes::Global;
                    frame.offsets.joint = joint.map(|v| length3(motion.vector(v)));
                }
                if motion.reflects() {
                    frame.cardinal_point = mirrored(frame.cardinal_point);
                }
            }
            frame_map.insert(*id, new);
            plan.frames.push(new);
            adds.push(Command::AddFrame { id: new, frame });
        }
        let mut shell_map = BTreeMap::new();
        for id in &shells {
            let source = &model.shells[id];
            let corners = source.nodes.map(|n| map[&n]);
            let mut key = corners;
            key.sort();
            if !shell_keys.insert(key) {
                plan.skipped += 1;
                continue;
            }
            let new = EntityId(next);
            next += 1;
            let mut shell = source.clone();
            shell.name = next_name(&source.name, &mut shell_names);
            shell.nodes = corners;
            if motion.reorients() {
                // Local x by default runs from the first corner to the
                // second; give it explicitly so it follows the copy.
                let x = source.local_x.unwrap_or_else(|| {
                    let [a, b] =
                        [0, 1].map(|k| model.nodes[&source.nodes[k]].position.map(|v| v.si()));
                    std::array::from_fn(|i| b[i] - a[i])
                });
                if motion.reflects() {
                    // A reflection turns the corner order around, and with
                    // it the normal. Reversing it again keeps the normal,
                    // and so pressure, on the side it was.
                    shell.nodes = [corners[0], corners[3], corners[2], corners[1]];
                    shell.local_x = Some(motion.vector(x));
                } else if source.local_x.is_some() {
                    shell.local_x = Some(motion.vector(x));
                }
            }
            shell_map.insert(*id, new);
            plan.shells.push(new);
            adds.push(Command::AddShell { id: new, shell });
        }
        if !loads {
            continue;
        }
        for (cid, case) in cases.iter_mut() {
            let before = (case.nodal.len(), case.member.len(), case.surface.len());
            let nodal: Vec<NodalLoad> = model.load_cases[cid]
                .nodal
                .iter()
                .filter(|l| nodes.contains(&l.node) && made.contains(&map[&l.node]))
                .map(|l| NodalLoad {
                    node: map[&l.node],
                    force: motion.vector(l.force.map(|v| v.si())).map(Force::from_si),
                    moment: motion.axial(l.moment.map(|v| v.si())).map(Moment::from_si),
                })
                .collect();
            let member: Vec<MemberLoad> = model.load_cases[cid]
                .member
                .iter()
                .filter_map(|l| Some(member_load(l, *frame_map.get(&l.member())?, motion)))
                .collect();
            let surface: Vec<SurfaceLoad> = model.load_cases[cid]
                .surface
                .iter()
                .filter_map(|l| {
                    Some(SurfaceLoad {
                        shell: *shell_map.get(&l.shell)?,
                        pressure: l.pressure,
                    })
                })
                .collect();
            case.nodal.extend(nodal);
            case.member.extend(member);
            case.surface.extend(surface);
            if before != (case.nodal.len(), case.member.len(), case.surface.len()) {
                cases_changed.insert(*cid);
            }
        }
    }
    plan.commands = adds;
    plan.commands
        .extend(cases_changed.into_iter().map(|id| Command::UpdateLoadCase {
            id,
            load_case: cases.remove(&id).unwrap(),
        }));
    Ok(plan)
}

/// What a frame's copy needs from the original when the copy turns or
/// reflects: its local y in global axes, and its joint offsets in global
/// axes when it has any.
struct FrameGeometry {
    y: [f64; 3],
    joint: Option<[[f64; 3]; 2]>,
}
fn frame_geometry_of(model: &Model, f: &Frame) -> Result<FrameGeometry> {
    let mut plain = f.clone();
    plain.cardinal_point = CardinalPoint::Centroid;
    let nodes = f.nodes.map(|n| model.nodes[&n].position.map(|v| v.si()));
    let fail = |e: String| ModelError::Invalid(format!("cannot replicate frame {:?}: {e}", f.name));
    let frame = crate::compile::solver_frame(
        model,
        &plain,
        [oa_core::NodeId(0), oa_core::NodeId(1)],
        0,
        0,
    )
    .map_err(fail)?;
    let ends = frame.ends(nodes).map_err(fail)?;
    let span = std::array::from_fn(|i| ends[1][i] - ends[0][i]);
    let [_, y, _] = frame.axes_along(span).map_err(fail)?;
    let joint = (!f.offsets.joint_si().iter().flatten().all(|v| *v == 0.0))
        .then(|| std::array::from_fn(|k| std::array::from_fn(|i| ends[k][i] - nodes[k][i])));
    Ok(FrameGeometry { y, joint })
}

/// The cardinal point that mirrors `p`. A reflected frame's local z points
/// the other way across the mirrored section, so left and right trade.
fn mirrored(p: CardinalPoint) -> CardinalPoint {
    use CardinalPoint::*;
    match p {
        BottomLeft => BottomRight,
        BottomRight => BottomLeft,
        MiddleLeft => MiddleRight,
        MiddleRight => MiddleLeft,
        TopLeft => TopRight,
        TopRight => TopLeft,
        other => other,
    }
}

/// A member load moved onto a copy. Positions run along the member as
/// before. Global components turn with the copy, moments as axial vectors.
/// In local axes a reflected copy's z runs the other way across the
/// mirrored member, so its z force and its x and y moments change sign.
fn member_load(load: &MemberLoad, member: EntityId, motion: &Motion) -> MemberLoad {
    let local_force = |v: [f64; 3]| {
        if motion.reflects() {
            [v[0], v[1], -v[2]]
        } else {
            v
        }
    };
    let local_moment = |v: [f64; 3]| {
        if motion.reflects() {
            [-v[0], -v[1], v[2]]
        } else {
            v
        }
    };
    match load {
        MemberLoad::Point {
            position,
            force,
            moment,
            axes,
            ..
        } => {
            let (f, m) = (force.map(|v| v.si()), moment.map(|v| v.si()));
            let (f, m) = match axes {
                Axes::Global => (motion.vector(f), motion.axial(m)),
                Axes::Local => (local_force(f), local_moment(m)),
            };
            MemberLoad::Point {
                member,
                position: *position,
                force: f.map(Force::from_si),
                moment: m.map(Moment::from_si),
                axes: *axes,
            }
        }
        MemberLoad::Distributed {
            start,
            end,
            start_load,
            end_load,
            axes,
            ..
        } => {
            let turn = |v: &[LineLoad; 3]| {
                let v = v.map(|x| x.si());
                match axes {
                    Axes::Global => motion.vector(v),
                    Axes::Local => local_force(v),
                }
                .map(LineLoad::from_si)
            };
            MemberLoad::Distributed {
                member,
                start: *start,
                end: *end,
                start_load: turn(start_load),
                end_load: turn(end_load),
                axes: *axes,
            }
        }
    }
}

/// One undo step that replicates and then connects the copies as drawing
/// does: each new frame is split at the nodes on its span, and every frame
/// a new node lands on is split there. Returns it with the plan, whose ids
/// are the ones the step adds before splitting.
pub fn replicate_and_connect(
    model: &Model,
    entities: &[EntityId],
    replication: &Replication,
    loads: bool,
) -> Result<(Command, ReplicationPlan)> {
    let plan = plan_replicate(model, entities, replication, loads)?;
    let mut commands = vec![Command::Replicate {
        entities: entities.to_vec(),
        replication: replication.clone(),
        loads,
    }];
    if !plan.frames.is_empty() {
        commands.push(Command::SplitFrames {
            frames: plan.frames.clone(),
            nodes: vec![],
        });
    }
    if !plan.nodes.is_empty() {
        commands.push(Command::SplitFrames {
            frames: vec![],
            nodes: plan.nodes.clone(),
        });
    }
    Ok((Command::Batch { commands }, plan))
}
