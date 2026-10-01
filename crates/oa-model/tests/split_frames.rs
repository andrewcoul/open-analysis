//! Splitting frames where nodes lie on their spans.
use oa_core::units::*;
use oa_model::*;

const L: f64 = 6.0;

struct Beam {
    editor: Editor,
    beam: EntityId,
    mid: EntityId,
    third: EntityId,
    case: EntityId,
    group: EntityId,
}

/// A 6 m pinned-roller beam along X with two loose nodes on its span, at
/// 3 m and 4 m, a load case, a 1.0 combination, and a group holding it.
fn beam() -> Beam {
    let mut editor = Editor::new(Model::default());
    let level = editor.model.base_level().unwrap();
    let mut id = || editor.model.allocate();
    let (mat, sec, i, j, mid, third, beam, case, combo, group) =
        (id(), id(), id(), id(), id(), id(), id(), id(), id(), id());
    let at = |x: f64| [Length::from_si(x), Length::ZERO, Length::ZERO];
    let mut pin = Node::new("I", level, at(0.0));
    pin.restrained = [true, true, true, true, false, true];
    let mut roller = Node::new("J", level, at(L));
    roller.restrained = [false, true, true, false, false, true];
    // Minor-axis end releases, which must stay at the ends.
    let mut frame = Frame::new("B1", [i, j], mat, sec);
    frame.releases[5] = true;
    frame.releases[11] = true;
    let commands = vec![
        Command::AddMaterial {
            id: mat,
            material: Library::starter().material("A992", "steel").unwrap(),
        },
        Command::AddSection {
            id: sec,
            section: Library::starter().section("W14x90", "beam").unwrap(),
        },
        Command::AddNode { id: i, node: pin },
        Command::AddNode {
            id: j,
            node: roller,
        },
        Command::AddNode {
            id: mid,
            node: Node::new("M", level, at(3.0)),
        },
        Command::AddNode {
            id: third,
            node: Node::new("T", level, at(4.0)),
        },
        Command::AddFrame { id: beam, frame },
        Command::AddLoadCase {
            id: case,
            load_case: LoadCase::new("D"),
        },
        Command::AddCombination {
            id: combo,
            combination: Combination {
                name: "1.0D".into(),
                terms: vec![(case, 1.0)],
            },
        },
        Command::AddGroup {
            id: group,
            group: Group {
                name: "Beams".into(),
                members: [beam].into(),
            },
        },
    ];
    editor.apply(Command::Batch { commands }).unwrap();
    Beam {
        editor,
        beam,
        mid,
        third,
        case,
        group,
    }
}

fn set_loads(b: &mut Beam, member: Vec<MemberLoad>) {
    let mut case = b.editor.model.load_cases[&b.case].clone();
    case.member = member;
    b.editor
        .apply(Command::UpdateLoadCase {
            id: b.case,
            load_case: case,
        })
        .unwrap();
}

fn line(w: f64) -> [LineLoad; 3] {
    [LineLoad::ZERO, LineLoad::ZERO, LineLoad::from_si(w)]
}

/// Every node's displacement under the one combination.
fn solve(model: &Model) -> Result<std::collections::BTreeMap<EntityId, [f64; 6]>, String> {
    let compiled = compile(model).map_err(|p| format!("{p:?}"))?;
    let results = oa_core::analyze_static(&compiled.solver, &oa_core::StaticOptions::default())
        .map_err(|e| e.to_string())?;
    let u = results.combinations[0].displacements.clone().unwrap();
    Ok(model
        .nodes
        .keys()
        .map(|id| (*id, u[compiled.mapping.node_index[id]]))
        .collect())
}

#[test]
fn nodes_on_the_span_are_found_in_order() {
    let b = beam();
    let on = split::nodes_on_span(&b.editor.model, b.beam, None);
    assert_eq!(on.len(), 2);
    assert_eq!(on[0].0, b.mid);
    assert!((on[0].1 - 3.0).abs() < 1e-12);
    assert_eq!(on[1].0, b.third);
    // A node off the axis by more than the tolerance is not on it.
    let mut m = b.editor.model.clone();
    m.nodes.get_mut(&b.mid).unwrap().position[1] = Length::from_si(0.01);
    assert_eq!(split::nodes_on_span(&m, b.beam, None).len(), 1);
}

#[test]
fn split_connects_the_nodes_keeps_end_releases_and_undoes() {
    let mut b = beam();
    let before = b.editor.model.clone();
    b.editor
        .apply(Command::SplitFrames {
            frames: vec![],
            nodes: vec![],
        })
        .unwrap();
    let m = &b.editor.model;
    assert_eq!(m.frames.len(), 3);
    let first = &m.frames[&b.beam];
    assert_eq!(first.name, "B1");
    assert_eq!(first.nodes[1], b.mid);
    let second = m.find::<Frame>("B1-2").unwrap();
    let third = m.find::<Frame>("B1-3").unwrap();
    assert_eq!(m.frames[&second].nodes, [b.mid, b.third]);
    assert_eq!(m.frames[&third].nodes[0], b.third);
    // The I-end release stays on the first piece, the J-end one on the last.
    assert!(first.releases[5] && !first.releases[11]);
    assert!(!m.frames[&second].releases.iter().any(|r| *r));
    assert!(!m.frames[&third].releases[5] && m.frames[&third].releases[11]);
    // Every piece joins the original's groups.
    assert_eq!(m.groups[&b.group].members.len(), 3);
    // Splitting again finds nothing left to split.
    assert!(split::plan_split_frames(m, &[], &[]).unwrap().is_empty());
    assert!(b.editor.undo().unwrap());
    assert_eq!(b.editor.model.frames, before.frames);
    assert_eq!(b.editor.model.groups, before.groups);
    assert!(b.editor.redo().unwrap());
    assert_eq!(b.editor.model.frames.len(), 3);
}

#[test]
fn split_limited_to_listed_nodes_and_frames() {
    let mut b = beam();
    b.editor
        .apply(Command::SplitFrames {
            frames: vec![b.beam],
            nodes: vec![b.mid],
        })
        .unwrap();
    assert_eq!(b.editor.model.frames.len(), 2);
    assert_eq!(b.editor.model.frames[&b.beam].nodes[1], b.mid);
    // A node is not a frame.
    let refused = b.editor.apply(Command::SplitFrames {
        frames: vec![b.mid],
        nodes: vec![],
    });
    assert!(matches!(refused, Err(ModelError::WrongKind { .. })));
}

#[test]
fn a_load_at_a_split_node_now_reaches_the_beam() {
    let mut b = beam();
    let p = -10_000.0;
    let mut case = b.editor.model.load_cases[&b.case].clone();
    case.nodal.push(NodalLoad {
        node: b.mid,
        force: [Force::ZERO, Force::ZERO, Force::from_si(p)],
        moment: [Moment::ZERO; 3],
    });
    b.editor
        .apply(Command::UpdateLoadCase {
            id: b.case,
            load_case: case,
        })
        .unwrap();
    // Unsplit, the loaded node is not part of the structure.
    assert!(solve(&b.editor.model).is_err());
    // The same force as a member load on the unsplit beam is the reference,
    // with the loose nodes held so that model solves.
    let mut reference = b.editor.model.clone();
    for n in [b.mid, b.third] {
        reference.nodes.get_mut(&n).unwrap().restrained = [true; 6];
    }
    let case = reference.load_cases.get_mut(&b.case).unwrap();
    case.nodal.clear();
    case.member.push(MemberLoad::Point {
        member: b.beam,
        position: Length::from_si(3.0),
        force: [Force::ZERO, Force::ZERO, Force::from_si(p)],
        moment: [Moment::ZERO; 3],
        axes: Axes::Global,
    });
    let reference = solve(&reference).unwrap();
    b.editor
        .apply(Command::SplitFrames {
            frames: vec![],
            nodes: vec![],
        })
        .unwrap();
    let split = solve(&b.editor.model).unwrap();
    assert!(split[&b.mid][2] < 0.0);
    for (n, u) in &reference {
        if *n == b.mid || *n == b.third {
            continue;
        }
        let (a, r) = (split[n][4], u[4]);
        assert!(r.abs() > 1e-6);
        assert!((a - r).abs() <= 1e-9 * r.abs(), "{a} vs {r}");
    }
}

#[test]
fn member_loads_follow_their_pieces_and_give_the_same_answer() {
    let mut b = beam();
    // A partial trapezoid across both joints and a point load past them.
    let beam_id = b.beam;
    set_loads(
        &mut b,
        vec![
            MemberLoad::Distributed {
                member: beam_id,
                start: Length::from_si(1.0),
                end: Length::from_si(5.0),
                start_load: line(-1_000.0),
                end_load: line(-3_000.0),
                axes: Axes::Global,
            },
            MemberLoad::Point {
                member: beam_id,
                position: Length::from_si(5.0),
                force: [Force::ZERO, Force::ZERO, Force::from_si(-5_000.0)],
                moment: [Moment::ZERO; 3],
                axes: Axes::Global,
            },
        ],
    );
    // Fix the loose nodes so the unsplit model solves; they carry nothing.
    let mut unsplit = b.editor.model.clone();
    for n in [b.mid, b.third] {
        unsplit.nodes.get_mut(&n).unwrap().restrained = [true; 6];
    }
    let reference = solve(&unsplit).unwrap();
    b.editor
        .apply(Command::SplitFrames {
            frames: vec![],
            nodes: vec![],
        })
        .unwrap();
    let m = &b.editor.model;
    let loads = &m.load_cases[&b.case].member;
    assert_eq!(loads.len(), 4);
    let second = m.find::<Frame>("B1-2").unwrap();
    let third = m.find::<Frame>("B1-3").unwrap();
    let MemberLoad::Distributed {
        member,
        start,
        end,
        start_load,
        end_load,
        ..
    } = &loads[1]
    else {
        panic!("expected the middle piece of the trapezoid");
    };
    assert_eq!(*member, second);
    assert!(start.si().abs() < 1e-12 && (end.si() - 1.0).abs() < 1e-12);
    assert!((start_load[2].si() + 2_000.0).abs() < 1e-9);
    assert!((end_load[2].si() + 2_500.0).abs() < 1e-9);
    let MemberLoad::Point {
        member, position, ..
    } = &loads[3]
    else {
        panic!("expected the point load");
    };
    assert_eq!(*member, third);
    assert!((position.si() - 1.0).abs() < 1e-12);
    // The same loads on the same beam: the end rotations do not change,
    // and the new joints now deflect with the beam.
    let split = solve(m).unwrap();
    let (i, j) = (m.frames[&b.beam].nodes[0], m.frames[&third].nodes[1]);
    for n in [i, j] {
        let (a, r) = (split[&n][4], reference[&n][4]);
        assert!(r.abs() > 1e-6);
        assert!((a - r).abs() <= 1e-9 * r.abs(), "{a} vs {r}");
    }
    assert!(split[&b.mid][2] < 0.0 && split[&b.third][2] < 0.0);
}

#[test]
fn end_offsets_stay_at_the_ends_and_joint_offsets_are_interpolated() {
    let mut b = beam();
    let mut frame = b.editor.model.frames[&b.beam].clone();
    frame.offsets.end = [Length::from_si(0.3), Length::from_si(0.2)];
    frame.offsets.rigid_zone = 1.0;
    let down = |z: f64| [Length::ZERO, Length::ZERO, Length::from_si(z)];
    frame.offsets.joint = [down(-0.2), down(-0.4)];
    b.editor
        .apply(Command::UpdateFrame { id: b.beam, frame })
        .unwrap();
    // A full-length uniform load along the moved member, which is longer
    // than the node-to-node span.
    let moved = compile::frame_length(&b.editor.model, b.beam).unwrap();
    assert!(moved > L);
    let beam_id = b.beam;
    set_loads(
        &mut b,
        vec![MemberLoad::Distributed {
            member: beam_id,
            start: Length::ZERO,
            end: Length::from_si(moved),
            start_load: line(-2_000.0),
            end_load: line(-2_000.0),
            axes: Axes::Global,
        }],
    );
    // Only the midpoint is split here; the other loose node is held.
    let mut held = b.editor.model.nodes[&b.third].clone();
    held.restrained = [true; 6];
    b.editor
        .apply(Command::UpdateNode {
            id: b.third,
            node: held,
        })
        .unwrap();
    let mut unsplit = b.editor.model.clone();
    unsplit.nodes.get_mut(&b.mid).unwrap().restrained = [true; 6];
    let reference = solve(&unsplit).unwrap();
    b.editor
        .apply(Command::SplitFrames {
            frames: vec![b.beam],
            nodes: vec![b.mid],
        })
        .unwrap();
    let m = &b.editor.model;
    let (first, second) = (
        &m.frames[&b.beam],
        &m.frames[&m.find::<Frame>("B1-2").unwrap()],
    );
    assert_eq!(first.offsets.end, [Length::from_si(0.3), Length::ZERO]);
    assert_eq!(second.offsets.end, [Length::ZERO, Length::from_si(0.2)]);
    assert_eq!(first.offsets.rigid_zone, 1.0);
    // Halfway along, the joint offset is halfway between the two ends'.
    assert!((first.offsets.joint[1][2].si() + 0.3).abs() < 1e-12);
    assert!((second.offsets.joint[0][2].si() + 0.3).abs() < 1e-12);
    assert!((second.offsets.joint[1][2].si() + 0.4).abs() < 1e-12);
    // The pieces lie on the original's moved line, so their lengths add up
    // to its length and the load is cut at the moved midpoint.
    let lengths: f64 = [b.beam, m.find::<Frame>("B1-2").unwrap()]
        .iter()
        .map(|id| compile::frame_length(m, *id).unwrap())
        .sum();
    assert!((lengths - moved).abs() < 1e-12);
    let MemberLoad::Distributed { end, .. } = &m.load_cases[&b.case].member[0] else {
        panic!("expected a distributed load");
    };
    assert!((end.si() - moved / 2.0).abs() < 1e-12);
    // The same beam under the same load: the supports turn alike.
    let split = solve(m).unwrap();
    for n in m.frames[&b.beam]
        .nodes
        .iter()
        .take(1)
        .chain(&second.nodes[1..])
    {
        let (a, r) = (split[n][4], reference[n][4]);
        assert!(r.abs() > 1e-6);
        assert!((a - r).abs() <= 1e-9 * r.abs(), "{a} vs {r}");
    }
}
