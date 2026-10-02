//! Replicating nodes, frames and shells: arrays, turns, mirrors, levels.
use oa_core::units::*;
use oa_model::replicate::{Replication, plan_replicate};
use oa_model::*;

fn m(x: f64) -> Length {
    Length::from_si(x)
}

struct Setup {
    editor: Editor,
    material: EntityId,
    section: EntityId,
    case: EntityId,
}

/// An empty model with steel, a W14x90, a load case "D" and a 1.0D
/// combination.
fn setup() -> Setup {
    let mut editor = Editor::new(Model::default());
    let (material, section, case, combo) = (
        editor.model.allocate(),
        editor.model.allocate(),
        editor.model.allocate(),
        editor.model.allocate(),
    );
    editor
        .apply(Command::Batch {
            commands: vec![
                Command::AddMaterial {
                    id: material,
                    material: Library::starter().material("A992", "steel").unwrap(),
                },
                Command::AddSection {
                    id: section,
                    section: Library::aisc().section("W14x90", "beam").unwrap(),
                },
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
            ],
        })
        .unwrap();
    Setup {
        editor,
        material,
        section,
        case,
    }
}

/// Adds nodes at the given points on the base level, the first fixed.
fn nodes(s: &mut Setup, names: &[&str], points: &[[f64; 3]], fixed: &[bool]) -> Vec<EntityId> {
    let level = s.editor.model.base_level().unwrap();
    let ids: Vec<EntityId> = points.iter().map(|_| s.editor.model.allocate()).collect();
    let commands = ids
        .iter()
        .zip(points)
        .zip(names)
        .zip(fixed)
        .map(|(((id, p), name), fixed)| {
            let node = if *fixed {
                Node::fixed(*name, level, p.map(m))
            } else {
                Node::new(*name, level, p.map(m))
            };
            Command::AddNode { id: *id, node }
        })
        .collect();
    s.editor.apply(Command::Batch { commands }).unwrap();
    ids
}

fn frame(s: &mut Setup, name: &str, ends: [EntityId; 2]) -> EntityId {
    let id = s.editor.model.allocate();
    let frame = Frame::new(name, ends, s.material, s.section);
    s.editor.apply(Command::AddFrame { id, frame }).unwrap();
    id
}

/// Every node's displacement under the one combination.
fn solve(model: &Model) -> std::collections::BTreeMap<EntityId, [f64; 6]> {
    let compiled = compile(model).unwrap_or_else(|p| panic!("{p:?}"));
    let results =
        oa_core::analyze_static(&compiled.solver, &oa_core::StaticOptions::default()).unwrap();
    let u = results.combinations[0].displacements.clone().unwrap();
    model
        .nodes
        .keys()
        .map(|id| (*id, u[compiled.mapping.node_index[id]]))
        .collect()
}

#[test]
fn a_linear_array_of_bays_shares_columns_and_undoes() {
    let mut s = setup();
    let n = nodes(
        &mut s,
        &["N1", "N2", "N3", "N4"],
        &[
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 4.0],
            [6.0, 0.0, 0.0],
            [6.0, 0.0, 4.0],
        ],
        &[true, false, true, false],
    );
    let c1 = frame(&mut s, "C1", [n[0], n[1]]);
    let b1 = frame(&mut s, "B1", [n[1], n[3]]);
    let c2 = frame(&mut s, "C2", [n[2], n[3]]);
    let before = s.editor.model.clone();
    let replication = Replication::Linear {
        offset: [m(6.0), m(0.0), m(0.0)],
        count: 3,
    };
    let plan = plan_replicate(&before, &[c1, b1, c2], &replication, true).unwrap();
    // Each bay's first column lands on the last bay's second one.
    assert_eq!(plan.skipped, 3);
    assert_eq!(plan.nodes.len(), 6);
    assert_eq!(plan.frames.len(), 6);
    s.editor
        .apply(Command::Replicate {
            entities: vec![c1, b1, c2],
            replication,
            loads: true,
        })
        .unwrap();
    let model = &s.editor.model;
    assert_eq!(model.nodes.len(), 10);
    assert_eq!(model.frames.len(), 9);
    // Copies take the next numbers after the names they copy.
    let last = model.find::<Frame>("B4").unwrap();
    let [i, j] = model.frames[&last].nodes;
    assert!((model.nodes[&i].position[0].si() - 18.0).abs() < 1e-12);
    assert!((model.nodes[&j].position[0].si() - 24.0).abs() < 1e-12);
    // Copied supports are supports.
    let bases = model
        .nodes
        .values()
        .filter(|n| n.restrained == [true; 6])
        .count();
    assert_eq!(bases, 5);
    // Replicating again onto itself adds nothing new.
    let again = plan_replicate(
        model,
        &[c1, b1, c2],
        &Replication::Linear {
            offset: [m(6.0), m(0.0), m(0.0)],
            count: 1,
        },
        true,
    )
    .unwrap();
    assert!(again.nodes.is_empty() && again.frames.is_empty());
    assert!(s.editor.undo().unwrap());
    assert_eq!(s.editor.model.nodes, before.nodes);
    assert_eq!(s.editor.model.frames, before.frames);
}

/// A skewed cantilever with offsets, a cardinal point, a rolled local y,
/// and loads of every kind, in global and local axes.
fn cantilever(s: &mut Setup) -> (EntityId, EntityId, EntityId) {
    let n = nodes(
        s,
        &["N1", "N2"],
        &[[1.0, 0.5, 0.0], [4.0, 1.5, 0.8]],
        &[true, false],
    );
    let id = s.editor.model.allocate();
    let mut f = Frame::new("F1", [n[0], n[1]], s.material, s.section);
    f.local_y = Some([0.3, 0.2, 1.0]);
    f.roll = Angle::from_si(0.4);
    f.cardinal_point = CardinalPoint::TopLeft;
    f.offsets.axes = Axes::Local;
    f.offsets.joint = [[m(0.0), m(0.1), m(0.05)], [m(0.0), m(0.1), m(0.05)]];
    s.editor.apply(Command::AddFrame { id, frame: f }).unwrap();
    let mut case = s.editor.model.load_cases[&s.case].clone();
    case.nodal.push(NodalLoad {
        node: n[1],
        force: [1e3, 2e3, -3e3].map(Force::from_si),
        moment: [100.0, -200.0, 300.0].map(Moment::from_si),
    });
    case.member.push(MemberLoad::Point {
        member: id,
        position: m(1.0),
        force: [500.0, -5e3, 3e3].map(Force::from_si),
        moment: [150.0, 250.0, -350.0].map(Moment::from_si),
        axes: Axes::Local,
    });
    case.member.push(MemberLoad::Distributed {
        member: id,
        start: m(0.5),
        end: m(2.5),
        start_load: [1e3, -2e3, -500.0].map(LineLoad::from_si),
        end_load: [0.0, -1e3, -1e3].map(LineLoad::from_si),
        axes: Axes::Global,
    });
    case.member.push(MemberLoad::Distributed {
        member: id,
        start: m(0.0),
        end: m(3.0),
        start_load: [0.0, 300.0, -800.0].map(LineLoad::from_si),
        end_load: [200.0, 300.0, -800.0].map(LineLoad::from_si),
        axes: Axes::Local,
    });
    s.editor
        .apply(Command::UpdateLoadCase {
            id: s.case,
            load_case: case,
        })
        .unwrap();
    (n[0], n[1], id)
}

/// The copy of the cantilever deflects as the original turned or mirrored
/// would: translations by `linear`, rotations by `linear` times its
/// determinant.
fn check_copy_deflects_like_the_original(replication: Replication, linear: [[f64; 3]; 3]) {
    let mut s = setup();
    let (_, tip, f) = cantilever(&mut s);
    s.editor
        .apply(Command::Replicate {
            entities: vec![f],
            replication,
            loads: true,
        })
        .unwrap();
    let model = &s.editor.model;
    assert_eq!(model.frames.len(), 2);
    let copy = model.find::<Frame>("F2").unwrap();
    let copy_tip = model.frames[&copy].nodes[1];
    let det = {
        let a = linear;
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let apply = |v: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|i| (0..3).map(|j| linear[i][j] * v[j]).sum())
    };
    let u = solve(model);
    let (a, b) = (u[&tip], u[&copy_tip]);
    let expect_t = apply([a[0], a[1], a[2]]);
    let expect_r = apply([a[3], a[4], a[5]]).map(|v| v * det);
    let scale = a.iter().map(|v| v.abs()).fold(0.0, f64::max);
    assert!(scale > 1e-6, "the original should deflect");
    for k in 0..3 {
        assert!(
            (b[k] - expect_t[k]).abs() < 1e-6 * scale,
            "u{k}: {b:?} vs {a:?}"
        );
        assert!(
            (b[3 + k] - expect_r[k]).abs() < 1e-6 * scale,
            "r{k}: {b:?} vs {a:?}"
        );
    }
}

#[test]
fn a_mirrored_copy_deflects_as_the_mirror_image() {
    // The plane x = 0.
    check_copy_deflects_like_the_original(
        Replication::Mirror {
            start: [m(0.0), m(0.0)],
            end: [m(0.0), m(1.0)],
        },
        [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    );
    // A skew plane through (−1, 0) at 30 degrees.
    let t = 30f64.to_radians();
    let (c, s) = ((2.0 * t).cos(), (2.0 * t).sin());
    check_copy_deflects_like_the_original(
        Replication::Mirror {
            start: [m(-1.0), m(0.0)],
            end: [m(-1.0 + t.cos()), m(t.sin())],
        },
        [[c, s, 0.0], [s, -c, 0.0], [0.0, 0.0, 1.0]],
    );
}

#[test]
fn a_turned_copy_deflects_as_the_original_turned() {
    let t = 100f64.to_radians();
    let (c, s) = (t.cos(), t.sin());
    check_copy_deflects_like_the_original(
        Replication::Radial {
            center: [m(-2.0), m(-3.0)],
            angle: Angle::from_si(t),
            count: 1,
        },
        [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
    );
}

#[test]
fn a_moved_copy_deflects_as_the_original() {
    check_copy_deflects_like_the_original(
        Replication::Linear {
            offset: [m(0.0), m(10.0), m(0.0)],
            count: 1,
        },
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    );
}

#[test]
fn a_radial_array_closes_on_the_original() {
    let mut s = setup();
    let n = nodes(
        &mut s,
        &["C", "R"],
        &[[0.0, 0.0, 3.0], [5.0, 0.0, 3.0]],
        &[false, false],
    );
    let spoke = frame(&mut s, "S1", [n[0], n[1]]);
    s.editor
        .apply(Command::Replicate {
            entities: vec![spoke],
            replication: Replication::Radial {
                center: [m(0.0), m(0.0)],
                angle: Angle::from_degrees(90.0),
                count: 4,
            },
            loads: false,
        })
        .unwrap();
    // The centre is shared, and the fourth turn lands on the original.
    let model = &s.editor.model;
    assert_eq!(model.frames.len(), 4);
    assert_eq!(model.nodes.len(), 5);
    assert!(model.frames.values().all(|f| f.nodes[0] == n[0]));
}

#[test]
fn copies_onto_levels_bind_to_those_levels() {
    let mut s = setup();
    let base = s.editor.model.base_level().unwrap();
    let levels: Vec<EntityId> = (1..=3).map(|_| s.editor.model.allocate()).collect();
    s.editor
        .apply(Command::Batch {
            commands: levels
                .iter()
                .enumerate()
                .map(|(k, id)| Command::AddLevel {
                    id: *id,
                    level: Level::new(format!("L{}", k + 1), m(4.0 * (k + 1) as f64)),
                })
                .collect(),
        })
        .unwrap();
    let level1 = levels[0];
    let a = s.editor.model.allocate();
    let b = s.editor.model.allocate();
    s.editor
        .apply(Command::Batch {
            commands: vec![
                Command::AddNode {
                    id: a,
                    node: Node::new("A", level1, [m(0.0), m(0.0), m(4.0)]),
                },
                Command::AddNode {
                    id: b,
                    node: Node::new("B", level1, [m(6.0), m(0.0), m(4.2)]),
                },
            ],
        })
        .unwrap();
    let beam = frame(&mut s, "B1", [a, b]);
    s.editor
        .apply(Command::Replicate {
            entities: vec![beam],
            replication: Replication::Levels {
                from: level1,
                to: vec![levels[1], levels[2]],
            },
            loads: true,
        })
        .unwrap();
    let model = &s.editor.model;
    let top = model.find::<Frame>("B3").unwrap();
    let [i, j] = model.frames[&top].nodes;
    assert_eq!(model.nodes[&i].level, levels[2]);
    assert!((model.nodes[&i].position[2].si() - 12.0).abs() < 1e-12);
    // The node 0.2 m above its level keeps that height above the copy's.
    assert!((model.nodes[&j].position[2].si() - 12.2).abs() < 1e-12);
    assert_eq!(model.nodes[&j].level, levels[2]);
    // Copying onto the level it came from, or the base twice, is refused.
    for to in [vec![level1], vec![base, base]] {
        assert!(
            plan_replicate(
                model,
                &[beam],
                &Replication::Levels { from: level1, to },
                true
            )
            .is_err()
        );
    }
}

#[test]
fn a_mirrored_shell_keeps_its_pressure_on_the_same_side() {
    let mut s = setup();
    let n = nodes(
        &mut s,
        &["N1", "N2", "N3", "N4"],
        &[
            [1.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [3.0, 2.0, 0.0],
            [1.0, 2.0, 0.0],
        ],
        &[true, true, true, true],
    );
    let id = s.editor.model.allocate();
    let shell = Shell {
        name: "S1".into(),
        nodes: [n[0], n[1], n[2], n[3]],
        material: s.material,
        thickness: m(0.2),
        formulation: Default::default(),
        drilling_ratio: 1e-3,
        local_x: None,
        modifiers: Default::default(),
    };
    let mut case = s.editor.model.load_cases[&s.case].clone();
    case.surface.push(SurfaceLoad {
        shell: id,
        pressure: Pressure::from_si(-5e3),
    });
    s.editor
        .apply(Command::Batch {
            commands: vec![
                Command::AddShell { id, shell },
                Command::UpdateLoadCase {
                    id: s.case,
                    load_case: case,
                },
            ],
        })
        .unwrap();
    s.editor
        .apply(Command::Replicate {
            entities: vec![id],
            replication: Replication::Mirror {
                start: [m(0.0), m(0.0)],
                end: [m(0.0), m(1.0)],
            },
            loads: true,
        })
        .unwrap();
    let model = &s.editor.model;
    let copy = model.find::<Shell>("S2").unwrap();
    let p = model.shells[&copy]
        .nodes
        .map(|n| model.nodes[&n].position.map(|v| v.si()));
    let (e1, e2) = (
        [p[1][0] - p[0][0], p[1][1] - p[0][1]],
        [p[3][0] - p[0][0], p[3][1] - p[0][1]],
    );
    // Its corners still run counterclockwise seen from above, so its
    // normal, and the pressure along it, still points up.
    assert!(e1[0] * e2[1] - e1[1] * e2[0] > 0.0);
    // Its local x is the mirror of the original's first edge.
    assert_eq!(model.shells[&copy].local_x, Some([-2.0, 0.0, 0.0]));
    assert_eq!(model.load_cases[&s.case].surface.len(), 2);
    assert_eq!(model.load_cases[&s.case].surface[1].shell, copy);
}

#[test]
fn refuses_what_it_cannot_copy() {
    let mut s = setup();
    let n = nodes(&mut s, &["N1"], &[[0.0, 0.0, 0.0]], &[false]);
    let model = &s.editor.model;
    let linear = |x: f64, count: u32| Replication::Linear {
        offset: [m(x), m(0.0), m(0.0)],
        count,
    };
    assert!(plan_replicate(model, &[], &linear(1.0, 1), true).is_err());
    assert!(plan_replicate(model, &[s.case], &linear(1.0, 1), true).is_err());
    assert!(plan_replicate(model, &[n[0]], &linear(0.0, 1), true).is_err());
    assert!(plan_replicate(model, &[n[0]], &linear(1.0, 0), true).is_err());
    let full_turn = Replication::Radial {
        center: [m(1.0), m(0.0)],
        angle: Angle::from_degrees(360.0),
        count: 1,
    };
    assert!(plan_replicate(model, &[n[0]], &full_turn, true).is_err());
    let point = Replication::Mirror {
        start: [m(1.0), m(0.0)],
        end: [m(1.0), m(0.0)],
    };
    assert!(plan_replicate(model, &[n[0]], &point, true).is_err());
    // A node copied onto one already there is that node.
    let mut s2 = setup();
    let n2 = nodes(
        &mut s2,
        &["A", "B"],
        &[[0.0; 3], [2.0, 0.0, 0.0]],
        &[false, false],
    );
    let plan = plan_replicate(&s2.editor.model, &[n2[0]], &linear(2.0, 1), true).unwrap();
    assert!(plan.commands.is_empty());
}

#[test]
fn replicate_round_trips_through_json() {
    let command = Command::Replicate {
        entities: vec![EntityId(4)],
        replication: Replication::Radial {
            center: [m(1.0), m(2.0)],
            angle: Angle::from_degrees(45.0),
            count: 3,
        },
        loads: false,
    };
    let json = serde_json::to_value(&command).unwrap();
    assert_eq!(json["command"], "replicate");
    assert_eq!(json["replication"]["type"], "radial");
    let back: Command = serde_json::from_value(json).unwrap();
    assert_eq!(back, command);
    // Loads come along unless told otherwise, and one copy is the default.
    let short: Command = serde_json::from_value(serde_json::json!({
        "command": "replicate", "entities": [4],
        "replication": {"type": "linear", "offset": [1, 0, 0]}
    }))
    .unwrap();
    assert!(matches!(
        short,
        Command::Replicate {
            loads: true,
            replication: Replication::Linear { count: 1, .. },
            ..
        }
    ));
}

#[test]
fn replicate_and_connect_splits_where_copies_land_on_spans() {
    let mut s = setup();
    // A girder along Y at x = 6, and a beam from x = 0 to it at y = 2.
    let n = nodes(
        &mut s,
        &["N1", "N2", "N3", "N4"],
        &[
            [6.0, 0.0, 0.0],
            [6.0, 10.0, 0.0],
            [0.0, 2.0, 0.0],
            [6.0, 2.0, 0.0],
        ],
        &[true, true, true, false],
    );
    frame(&mut s, "G1", [n[0], n[1]]);
    let beam = frame(&mut s, "B1", [n[2], n[3]]);
    s.editor
        .apply(Command::SplitFrames {
            frames: vec![],
            nodes: vec![],
        })
        .unwrap();
    assert_eq!(s.editor.model.frames.len(), 3);
    // Two more beams at 3 m centres land on the girder's span.
    let (command, plan) = replicate::replicate_and_connect(
        &s.editor.model,
        &[beam],
        &Replication::Linear {
            offset: [m(0.0), m(3.0), m(0.0)],
            count: 2,
        },
        true,
    )
    .unwrap();
    assert_eq!(plan.frames.len(), 2);
    s.editor.apply(command).unwrap();
    let model = &s.editor.model;
    // The girder is now in four pieces, joined to every beam.
    assert_eq!(model.frames.len(), 7);
    for id in &plan.frames {
        let end = model.frames[id].nodes[1];
        let joined = model
            .frames
            .iter()
            .filter(|(f, frame)| *f != id && frame.nodes.contains(&end))
            .count();
        assert_eq!(joined, 2, "{}", model.frames[id].name);
    }
    assert!(s.editor.undo().unwrap());
    assert_eq!(s.editor.model.frames.len(), 3);
}
