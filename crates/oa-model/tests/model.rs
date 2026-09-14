//! Acceptance tests for the model layer, one per plan phase.
use oa_core::units::*;
use oa_model::*;

fn steel() -> Material {
    Library::starter().material("A992", "steel").unwrap()
}
fn section() -> Section {
    Library::starter().section("W14x90", "column").unwrap()
}

/// Two-storey, one-bay portal frame built through commands, Y up.
fn portal(editor: &mut Editor) -> (EntityId, EntityId) {
    let m = &mut editor.model;
    let mat = m.allocate();
    let sec = m.allocate();
    editor
        .apply(Command::AddMaterial {
            id: mat,
            material: steel(),
        })
        .unwrap();
    editor
        .apply(Command::AddSection {
            id: sec,
            section: section(),
        })
        .unwrap();
    let mut ids = vec![];
    for (i, (x, y)) in [
        (0.0, 0.0),
        (6.0, 0.0),
        (0.0, 4.0),
        (6.0, 4.0),
        (0.0, 8.0),
        (6.0, 8.0),
    ]
    .into_iter()
    .enumerate()
    {
        let id = editor.model.allocate();
        let p = [Length::from_si(x), Length::from_si(y), Length::ZERO];
        let node = if i < 2 {
            Node::fixed(format!("N{i}"), p)
        } else {
            Node::new(format!("N{i}"), p)
        };
        editor.apply(Command::AddNode { id, node }).unwrap();
        ids.push(id);
    }
    let mut frames = vec![];
    for (name, a, b) in [
        ("C1", 0, 2),
        ("C2", 1, 3),
        ("C3", 2, 4),
        ("C4", 3, 5),
        ("B1", 2, 3),
        ("B2", 4, 5),
    ] {
        let id = editor.model.allocate();
        editor
            .apply(Command::AddFrame {
                id,
                frame: Frame::new(name, [ids[a], ids[b]], mat, sec),
            })
            .unwrap();
        frames.push(id);
    }
    let case = editor.model.allocate();
    editor
        .apply(Command::AddLoadCase {
            id: case,
            load_case: LoadCase {
                nodal: vec![
                    NodalLoad {
                        node: ids[2],
                        force: [Force::from_si(10_000.0), Force::ZERO, Force::ZERO],
                        moment: [Moment::ZERO; 3],
                    },
                    NodalLoad {
                        node: ids[4],
                        force: [Force::from_si(20_000.0), Force::ZERO, Force::ZERO],
                        moment: [Moment::ZERO; 3],
                    },
                ],
                ..LoadCase::new("wind")
            },
        })
        .unwrap();
    let combo = editor.model.allocate();
    editor
        .apply(Command::AddCombination {
            id: combo,
            combination: Combination {
                name: "1.0W".into(),
                terms: vec![(case, 1.0)],
            },
        })
        .unwrap();
    let group = editor.model.allocate();
    editor
        .apply(Command::AddGroup {
            id: group,
            group: Group {
                name: "roof".into(),
                members: [ids[4], ids[5], frames[5]].into_iter().collect(),
            },
        })
        .unwrap();
    (ids[4], group)
}

#[test]
fn m0_round_trip_build_compile_solve_reattach() {
    let mut editor = Editor::new(Model::default());
    let (roof_node, roof_group) = portal(&mut editor);
    let compiled = compile(&editor.model).unwrap();
    assert_eq!(compiled.solver.nodes.len(), 6);
    assert_eq!(compiled.solver.frames.len(), 6);
    let options = oa_core::StaticOptions::default();
    let mut store = oa_results::ResultStore::create_in_memory(&compiled.solver, &options).unwrap();
    oa_core::analyze_static_into(&compiled.solver, &options, &mut store).unwrap();
    oa_model::store::attach(&compiled, &store).unwrap();
    // Results come back through the mapping, by entity and by group.
    let index = compiled.mapping.node_index[&roof_node];
    let sway = store.envelope_displacement(index, 0).unwrap();
    assert!(sway.maximum.value > 0.0);
    assert_eq!(sway.maximum.combination, "1.0W");
    let group = compiled.group_indices(&editor.model, roof_group);
    assert_eq!(group.nodes.len(), 2);
    assert_eq!(group.frames.len(), 1);
    let envelopes = store.envelope_displacements(&group.nodes, 0).unwrap();
    assert_eq!(envelopes.len(), 2);
    assert_eq!(compiled.mapping.node_id[index], Some(roof_node));
    // Editing the model makes the store stale.
    editor
        .apply(Command::SetGravity {
            gravity: Acceleration::from_si(9.81),
        })
        .unwrap();
    let recompiled = compile(&editor.model).unwrap();
    assert!(matches!(
        oa_model::store::attach(&recompiled, &store),
        Err(oa_model::store::StoreError::StaleResults { .. })
    ));
}

#[test]
fn m0_solver_fixtures_compile_identically_through_the_model_layer() {
    let reference: serde_json::Value = serde_json::from_str(include_str!(
        "../../oa-core/tests/fixtures/pynite_reference.json"
    ))
    .unwrap();
    let cases = reference["cases"].as_array().unwrap();
    assert!(cases.len() >= 24);
    for case in cases {
        let solver: oa_core::Model =
            serde_json::from_value(case["request"]["model"].clone()).unwrap();
        let model = Model::from_solver(&solver);
        let compiled = compile(&model).unwrap_or_else(|p| panic!("{}: {:?}", case["name"], p));
        assert_eq!(
            serde_json::to_string(&compiled.solver).unwrap(),
            serde_json::to_string(&solver).unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(compiled.content_hash(), solver.content_hash());
    }
}

#[test]
fn m0_problems_name_entities_not_indices() {
    let mut model = Model::default();
    let mat = model.insert(steel());
    let sec = model.insert(section());
    let a = model.insert(Node::fixed("A", [Length::ZERO; 3]));
    let missing = EntityId(999);
    model.insert(Frame::new("F", [a, missing], mat, sec));
    model.insert(Node::new(
        "A",
        [Length::from_si(1.0), Length::ZERO, Length::ZERO],
    ));
    let problems = compile(&model).unwrap_err();
    let text: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
    assert!(
        text.iter()
            .any(|t| t.contains("\"F\"") && t.contains("#999")),
        "{text:?}"
    );
    assert!(
        text.iter()
            .any(|t| t.contains("\"A\"") && t.contains("also used")),
        "{text:?}"
    );
    assert!(problems.iter().all(|p| p.entity.is_some()));
}

#[test]
fn m0_auto_master_sits_at_mass_centroid_and_matches_explicit_master() {
    let build = |master: bool| {
        let mut model = Model::default();
        let mat = model.insert(steel());
        let sec = model.insert(section());
        let mut tops = vec![];
        for (i, z) in [-2.0, 2.0].into_iter().enumerate() {
            let base = model.insert(Node::fixed(
                format!("B{i}"),
                [Length::ZERO, Length::ZERO, Length::from_si(z)],
            ));
            let mut top = Node::new(
                format!("T{i}"),
                [Length::ZERO, Length::from_si(3.0), Length::from_si(z)],
            );
            top.mass[0] = Mass::from_si(if i == 0 { 100.0 } else { 300.0 });
            let top = model.insert(top);
            model.insert(Frame::new(format!("C{i}"), [base, top], mat, sec));
            tops.push(top);
        }
        let explicit = master.then(|| {
            model.insert(Node::new(
                "M",
                [Length::ZERO, Length::from_si(3.0), Length::ZERO],
            ))
        });
        model.insert(Diaphragm {
            name: "L1".into(),
            master: explicit,
            nodes: tops,
            normal: Axis::Y,
        });
        compile(&model).unwrap()
    };
    let auto = build(false);
    let (diaphragm, index) = auto.mapping.synthetic_masters[0];
    assert_eq!(auto.mapping.node_id[index], None);
    assert_eq!(auto.solver.diaphragms[0].master.0, index);
    let position = auto.solver.nodes[index].xyz();
    assert!((position[2] - 1.0).abs() < 1e-12, "{position:?}");
    assert!(
        auto.mapping
            .node_id
            .iter()
            .flatten()
            .all(|id| *id != diaphragm)
    );
    let options = oa_core::ModalOptions {
        modes: 2,
        ..Default::default()
    };
    let a = oa_core::analyze_modal(&auto.solver, &options).unwrap();
    let b = oa_core::analyze_modal(&build(true).solver, &options).unwrap();
    for (x, y) in a.modes.iter().zip(&b.modes) {
        assert!((x.eigenvalue / y.eigenvalue - 1.0).abs() < 1e-9);
    }
}

/// Deterministic xorshift so the property test is reproducible without a dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn m1_random_command_sequences_undo_to_the_start_and_redo_to_the_end() {
    // Build the portal, then start a fresh history so undo stops at `start`.
    let mut builder = Editor::new(Model::default());
    portal(&mut builder);
    let mut editor = Editor::new(builder.model);
    let start = editor.model.clone();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut applied = 0;
    for _ in 0..300 {
        let m = &editor.model;
        let nodes: Vec<_> = m.nodes.keys().copied().collect();
        let frames: Vec<_> = m.frames.keys().copied().collect();
        let command = match rng.below(6) {
            0 => {
                let id = editor.model.allocate();
                let n = editor.model.nodes.len();
                Command::AddNode {
                    id,
                    node: Node::new(
                        format!("R{}", id.0),
                        [
                            Length::from_si(rng.below(10) as f64),
                            Length::from_si(rng.below(10) as f64),
                            Length::from_si(n as f64),
                        ],
                    ),
                }
            }
            1 => {
                let id = nodes[rng.below(nodes.len())];
                let mut node = editor.model.nodes[&id].clone();
                node.mass[1] = Mass::from_si(rng.below(1000) as f64);
                Command::UpdateNode { id, node }
            }
            2 => Command::RemoveNode {
                id: nodes[rng.below(nodes.len())],
            },
            3 => {
                let id = editor.model.allocate();
                let mat = *editor.model.materials.keys().next().unwrap();
                let sec = *editor.model.sections.keys().next().unwrap();
                Command::AddFrame {
                    id,
                    frame: Frame::new(
                        format!("RF{}", id.0),
                        [nodes[rng.below(nodes.len())], nodes[rng.below(nodes.len())]],
                        mat,
                        sec,
                    ),
                }
            }
            4 => Command::RemoveFrame {
                id: frames[rng.below(frames.len())],
            },
            _ => {
                let id = editor.model.allocate();
                let members = (0..rng.below(4))
                    .map(|_| nodes[rng.below(nodes.len())])
                    .collect();
                Command::AddGroup {
                    id,
                    group: Group {
                        name: format!("G{}", id.0),
                        members,
                    },
                }
            }
        };
        // Some of these are expected to fail (removing a referenced node); a
        // failed command must leave the model untouched and add no history.
        let before = editor.model.clone();
        match editor.apply(command) {
            Ok(()) => applied += 1,
            Err(_) => assert_eq!(editor.model, before),
        }
    }
    assert!(applied > 100, "only {applied} commands applied");
    let end = editor.model.clone();
    let mut undone = 0;
    while editor.undo().unwrap() {
        undone += 1;
    }
    assert_eq!(undone, applied);
    // next_id only ever grows, and that is the one field undo does not restore.
    let mut restored = editor.model.clone();
    restored.next_id = start.next_id;
    assert_eq!(restored, start);
    while editor.redo().unwrap() {}
    assert_eq!(editor.model, end);
}

#[test]
fn m1_batches_are_atomic_and_removal_refuses_referenced_entities() {
    let mut editor = Editor::new(Model::default());
    portal(&mut editor);
    let base = editor.model.find::<Node>("N0").unwrap();
    let new_node = editor.model.allocate();
    let before = editor.model.clone();
    let err = editor
        .apply(Command::Batch {
            commands: vec![
                Command::AddNode {
                    id: new_node,
                    node: Node::new("extra", [Length::ZERO; 3]),
                },
                Command::RemoveNode { id: base },
            ],
        })
        .unwrap_err();
    assert!(matches!(
        err,
        ModelError::Batch {
            index: 1,
            ref source
        } if matches!(**source, ModelError::Referenced { .. })
    ));
    assert_eq!(editor.model, before, "rolled back");
    let dup = editor.model.allocate();
    assert!(matches!(
        editor.apply(Command::AddNode {
            id: dup,
            node: Node::new("N0", [Length::ZERO; 3]),
        }),
        Err(ModelError::DuplicateName { .. })
    ));
    let mat = editor.model.find::<Material>("steel").unwrap();
    assert!(matches!(
        editor.apply(Command::RemoveNode { id: mat }),
        Err(ModelError::WrongKind { .. })
    ));
}

#[test]
fn m2_format_fixture_loads_round_trips_and_rejects_newer_versions() {
    let text = include_str!("fixtures/format_v1.json");
    let model = from_json(text).unwrap();
    assert_eq!(model.format_version, FORMAT_VERSION);
    assert_eq!(model.nodes.len(), 2);
    let compiled = compile(&model).unwrap();
    assert_eq!(compiled.solver.combinations[0].name, "service");
    let again = from_json(&to_json(&model)).unwrap();
    assert_eq!(again, model);
    let mut newer: serde_json::Value = serde_json::from_str(text).unwrap();
    newer["format_version"] = serde_json::json!(FORMAT_VERSION + 1);
    assert!(from_json(&newer.to_string()).is_err());
    let mut missing: serde_json::Value = serde_json::from_str(text).unwrap();
    missing.as_object_mut().unwrap().remove("format_version");
    assert!(from_json(&missing.to_string()).is_err());
}

#[test]
fn m3_library_copies_carry_provenance_and_survive_updates() {
    let library = Library::starter();
    assert!(library.section_designations().contains(&"IPE200"));
    let s = library.section("IPE200", "beam").unwrap();
    let p = s.provenance.as_ref().unwrap();
    assert_eq!(p.library, "oa-starter");
    assert_eq!(p.designation, "IPE200");
    assert!((s.area.si() - 0.002848).abs() < 1e-12);
    assert!(library.section("W99x999", "x").is_none());
    let model = from_json(&to_json(&{
        let mut m = Model::default();
        m.insert(s);
        m
    }))
    .unwrap();
    assert!(model.sections.values().next().unwrap().provenance.is_some());
}

#[test]
fn m4_groups_resolve_members_and_drop_removed_entities() {
    let mut editor = Editor::new(Model::default());
    let (_, group) = portal(&mut editor);
    assert_eq!(editor.model.group_members(group).len(), 3);
    let roof_beam = editor.model.find::<Frame>("B2").unwrap();
    editor
        .apply(Command::RemoveFrame { id: roof_beam })
        .unwrap();
    assert_eq!(editor.model.group_members(group).len(), 2);
    editor.undo().unwrap();
    // Undo restores the frame and its group membership.
    assert_eq!(editor.model.group_members(group).len(), 3);
    assert!(editor.model.groups[&group].members.contains(&roof_beam));
    let missing = editor.model.allocate();
    assert!(matches!(
        editor.apply(Command::UpdateGroup {
            id: group,
            group: Group {
                name: "roof".into(),
                members: [missing].into_iter().collect(),
            },
        }),
        Err(ModelError::Dangling { .. })
    ));
}

#[test]
fn m1_journal_replays_an_interrupted_session() {
    let dir = std::env::temp_dir().join(format!("oa-model-journal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.sqlite");
    let start = Model::default();
    let end = {
        let journal = oa_model::store::Journal::open(&path).unwrap();
        let mut editor = Editor::new(start.clone()).with_journal(journal);
        portal(&mut editor);
        editor.undo().unwrap();
        editor.model.clone()
    };
    let journal = oa_model::store::Journal::open(&path).unwrap();
    let mut replayed = start;
    for command in journal.replay().unwrap() {
        command.apply(&mut replayed).unwrap();
    }
    replayed.next_id = end.next_id;
    assert_eq!(replayed, end);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn m5_json_api_applies_commands_compiles_and_solves() {
    let base = include_str!("fixtures/format_v1.json");
    let commands = serde_json::json!([
        {"command": "add_node", "id": 20, "node": {"name": "mid", "position": [1.5, 0, 0]}},
        {"command": "set_gravity", "gravity": 9.81}
    ]);
    let updated = oa_model::api::apply_commands_json(base, &commands.to_string()).unwrap();
    let model = from_json(&updated).unwrap();
    assert_eq!(model.nodes.len(), 3);
    assert!((model.gravity.si() - 9.81).abs() < 1e-12);
    let compiled: serde_json::Value =
        serde_json::from_str(&oa_model::api::compile_json(base).unwrap()).unwrap();
    assert_eq!(compiled["solver"]["nodes"].as_array().unwrap().len(), 2);
    let response: serde_json::Value =
        serde_json::from_str(&oa_model::api::solve_json(base, "{}").unwrap()).unwrap();
    let tip = response["combinations"][0]["displacements"][1][1]
        .as_f64()
        .unwrap();
    assert!((tip / (-1000.0 * 27.0 / (3.0 * 200e9 * 4e-5)) - 1.0).abs() < 1e-9);
    let bad = serde_json::json!([{"command": "remove_node", "id": 1}]);
    assert!(oa_model::api::apply_commands_json(base, &bad.to_string()).is_err());
}
