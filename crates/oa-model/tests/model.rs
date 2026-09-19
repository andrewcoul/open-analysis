//! Acceptance tests for the model layer, one per plan phase.
use oa_core::units::*;
use oa_model::*;

fn steel() -> Material {
    Library::starter().material("A992", "steel").unwrap()
}
fn section() -> Section {
    Library::starter().section("W14x90", "column").unwrap()
}

/// Two-storey, one-bay portal frame built through commands, Z up, with a
/// level per floor and every node bound to its floor.
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
    let base = editor.model.base_level().unwrap();
    let mut levels = vec![base];
    for (name, z) in [("L1", 4.0), ("L2", 8.0)] {
        let id = editor.model.allocate();
        editor
            .apply(Command::AddLevel {
                id,
                level: Level::new(name, Length::from_si(z)),
            })
            .unwrap();
        levels.push(id);
    }
    let mut ids = vec![];
    for (i, (x, z)) in [
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
        let p = [Length::from_si(x), Length::ZERO, Length::from_si(z)];
        let level = levels[i / 2];
        let node = if i < 2 {
            Node::fixed(format!("N{i}"), level, p)
        } else {
            Node::new(format!("N{i}"), level, p)
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
    let base = model.base_level().unwrap();
    let mat = model.insert(steel());
    let sec = model.insert(section());
    let a = model.insert(Node::fixed("A", base, [Length::ZERO; 3]));
    let missing = EntityId(999);
    model.insert(Frame::new("F", [a, missing], mat, sec));
    model.insert(Node::new(
        "A",
        base,
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
        let level = model.base_level().unwrap();
        let mat = model.insert(steel());
        let sec = model.insert(section());
        let mut tops = vec![];
        for (i, z) in [-2.0, 2.0].into_iter().enumerate() {
            let base = model.insert(Node::fixed(
                format!("B{i}"),
                level,
                [Length::ZERO, Length::ZERO, Length::from_si(z)],
            ));
            let mut top = Node::new(
                format!("T{i}"),
                level,
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
                level,
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
                let level = editor.model.base_level().unwrap();
                Command::AddNode {
                    id,
                    node: Node::new(
                        format!("R{}", id.0),
                        level,
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
    let level = editor.model.base_level().unwrap();
    let new_node = editor.model.allocate();
    let before = editor.model.clone();
    let err = editor
        .apply(Command::Batch {
            commands: vec![
                Command::AddNode {
                    id: new_node,
                    node: Node::new("extra", level, [Length::ZERO; 3]),
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
            node: Node::new("N0", level, [Length::ZERO; 3]),
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
    // Version 1 had no levels: migration adds one base datum at zero and
    // binds every node to it with its Z as the offset, inventing no floors
    // and changing no coordinates.
    assert_eq!(model.levels.len(), 1);
    let base = model.base_level().unwrap();
    assert_eq!(base, EntityId(9), "a fresh id past the highest in use");
    assert_eq!(model.next_id, 10);
    assert!(model.nodes.values().all(|n| n.level == base));
    assert_eq!(model.nodes[&EntityId(2)].position[0].si(), 3.0);
    let v2 = from_json(include_str!("fixtures/format_v2.json")).unwrap();
    assert_eq!(model, v2, "the migrated document is the version 2 fixture");
    let compiled = compile(&model).unwrap();
    assert_eq!(compiled.solver.combinations[0].name, "service");
    assert_eq!(
        compiled.content_hash(),
        oa_model::compile(&Model::from_solver(&compiled.solver))
            .unwrap()
            .content_hash(),
        "levels leave the compiled model untouched"
    );
    let again = from_json(&to_json(&model)).unwrap();
    assert_eq!(again, model);
    // A version 2 document must bind every node to a level that exists.
    let mut unbound: serde_json::Value = serde_json::from_str(&to_json(&model)).unwrap();
    unbound["nodes"]["1"]["level"] = serde_json::json!(999);
    assert!(matches!(
        from_json(&unbound.to_string()),
        Err(oa_model::format::FormatError::Corrupt(_))
    ));
    let mut no_levels: serde_json::Value = serde_json::from_str(&to_json(&model)).unwrap();
    no_levels["levels"] = serde_json::json!({});
    no_levels["nodes"] = serde_json::json!({});
    assert!(matches!(
        from_json(&no_levels.to_string()),
        Err(oa_model::format::FormatError::Corrupt(_))
    ));
    // A version 1 document with no id left for the base level is corrupt,
    // not a panic, whether the allocator or a table key is exhausted.
    let mut spent: serde_json::Value = serde_json::from_str(text).unwrap();
    spent["next_id"] = serde_json::json!(u64::MAX);
    assert!(matches!(
        from_json(&spent.to_string()),
        Err(oa_model::format::FormatError::Corrupt(_))
    ));
    let mut spent: serde_json::Value = serde_json::from_str(text).unwrap();
    let node = spent["nodes"]["1"].clone();
    spent["nodes"][u64::MAX.to_string()] = node;
    assert!(matches!(
        from_json(&spent.to_string()),
        Err(oa_model::format::FormatError::Corrupt(_))
    ));
    let mut newer: serde_json::Value = serde_json::from_str(text).unwrap();
    newer["format_version"] = serde_json::json!(FORMAT_VERSION + 1);
    assert!(from_json(&newer.to_string()).is_err());
    let mut missing: serde_json::Value = serde_json::from_str(text).unwrap();
    missing.as_object_mut().unwrap().remove("format_version");
    assert!(from_json(&missing.to_string()).is_err());
}

/// Coordinates and load positions are converted from display units one by
/// one, so a full-span load on a member away from the origin can land a few
/// ulps past the length the solver computes. Compilation snaps it back.
#[test]
fn full_span_loads_converted_from_feet_compile() {
    let us = UnitSystem::UsCustomary;
    let ft = |v: f64| Length::from_si(us.from_display(Role::Length, v));
    let mut m = Model::default();
    let level = m.base_level().unwrap();
    let mat = m.insert(steel());
    let sec = m.insert(section());
    let a = m.insert(Node::fixed("A", level, [ft(48.0), Length::ZERO, Length::ZERO]));
    let b = m.insert(Node::new("B", level, [ft(60.0), Length::ZERO, Length::ZERO]));
    let beam = m.insert(Frame::new("B1", [a, b], mat, sec));
    let mut case = LoadCase::new("D");
    let w = LineLoad::from_kips_per_foot(-1.0);
    case.member.push(MemberLoad::Distributed {
        member: beam,
        start: Length::ZERO,
        end: ft(12.0),
        start_load: [LineLoad::ZERO, w, LineLoad::ZERO],
        end_load: [LineLoad::ZERO, w, LineLoad::ZERO],
        axes: Axes::Global,
    });
    case.member.push(MemberLoad::Point {
        member: beam,
        position: ft(12.0),
        force: [Force::ZERO, Force::from_kips(-1.0), Force::ZERO],
        moment: [Moment::ZERO; 3],
        axes: Axes::Global,
    });
    m.insert(case);
    let span = (ft(60.0).si() - ft(48.0).si()).abs();
    assert_ne!(
        ft(12.0).si(),
        span,
        "the case only matters when they differ"
    );
    let compiled = compile(&m).expect("a 12 ft load fits a 12 ft member");
    let oa_core::MemberLoad::Distributed { end, .. } = compiled.solver.load_cases[0].member[0]
    else {
        panic!()
    };
    assert_eq!(end.si(), span);
}

#[test]
fn m3_library_copies_carry_provenance_and_survive_updates() {
    let library = Library::starter();
    assert!(library.section_designations().contains(&"W12x26"));
    let s = library.section("W12x26", "beam").unwrap();
    let p = s.provenance.as_ref().unwrap();
    assert_eq!(p.library, "oa-starter");
    assert_eq!(p.designation, "W12x26");
    // The library is written in AISC units; the copy is SI (7.65 in²).
    assert!((s.area.si() - 7.65 * 0.0254_f64.powi(2)).abs() < 1e-12);
    assert!((s.iz.si() - 204.0 * 0.0254_f64.powi(4)).abs() < 1e-15);
    let steel = library.material("A992", "steel").unwrap();
    assert!((steel.young.si() - 29_000.0 * 6.894_757_293_168e6).abs() < 1e3);
    assert!(
        (steel.density.si() - 7848.6).abs() < 0.5,
        "{}",
        steel.density.si()
    );
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
        // A level move is journalled as the one command that was issued and
        // replays to the same geometry.
        let l1 = editor.model.find::<Level>("L1").unwrap();
        editor
            .apply(Command::SetLevelElevation {
                id: l1,
                elevation: Length::from_si(4.5),
                scope: ElevationScope::ThisAndAbove,
            })
            .unwrap();
        editor.undo().unwrap();
        editor.redo().unwrap();
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
        {"command": "add_node", "id": 20, "node": {"name": "mid", "level": 9, "position": [1.5, 0, 0]}},
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
    // A node without a level is refused at the JSON boundary.
    let unbound = serde_json::json!([
        {"command": "add_node", "id": 21, "node": {"name": "loose", "position": [0, 0, 0]}}
    ]);
    assert!(oa_model::api::apply_commands_json(base, &unbound.to_string()).is_err());
}

#[test]
fn journal_records_only_accepted_commands_and_replays_cleanly() {
    let dir = std::env::temp_dir().join(format!("oa-model-journal-reject-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.sqlite");
    {
        let journal = oa_model::store::Journal::open(&path).unwrap();
        let mut editor = Editor::new(Model::default()).with_journal(journal);
        let level = editor.model.base_level().unwrap();
        let a = Node::new("A", level, [Length::ZERO; 3]);
        editor
            .apply(Command::AddNode {
                id: EntityId(11),
                node: a.clone(),
            })
            .unwrap();
        // Rejected: duplicate name. Must leave no trace in the journal.
        assert!(matches!(
            editor.apply(Command::AddNode {
                id: EntityId(12),
                node: a,
            }),
            Err(ModelError::DuplicateName { .. })
        ));
        // Rejected batch: the second command dangles, so the whole batch rolls back.
        assert!(
            editor
                .apply(Command::Batch {
                    commands: vec![
                        Command::AddNode {
                            id: EntityId(15),
                            node: Node::new("C", level, [Length::ZERO; 3]),
                        },
                        Command::RemoveNode { id: EntityId(99) },
                    ],
                })
                .is_err()
        );
        editor
            .apply(Command::AddNode {
                id: EntityId(13),
                node: Node::new("B", level, [Length::ZERO; 3]),
            })
            .unwrap();
        assert!(editor.undo().unwrap());
        assert!(editor.redo().unwrap());
        assert_eq!(editor.model.nodes.len(), 2);
    }
    let journal = oa_model::store::Journal::open(&path).unwrap();
    let commands = journal.replay().unwrap();
    assert_eq!(commands.len(), 4, "add A, add B, undo, redo");
    let mut recovered = Model::default();
    for command in commands {
        command.apply(&mut recovered).unwrap();
    }
    assert_eq!(recovered.nodes.len(), 2);
    assert_eq!(recovered.nodes[&EntityId(11)].name, "A");
    assert_eq!(recovered.nodes[&EntityId(13)].name, "B");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn loading_checks_identity_invariants_and_repairs_a_stale_allocator() {
    let mut editor = Editor::new(Model::default());
    portal(&mut editor);
    let model = editor.model;
    let count = model.nodes.len();

    // A stale allocator is moved past the highest id instead of handing out
    // ids that are already taken.
    let mut stale = model.clone();
    stale.next_id = 1;
    let mut loaded = from_json(&to_json(&stale)).unwrap();
    assert_eq!(loaded.next_id, model.next_id);
    let level = loaded.base_level().unwrap();
    let fresh = loaded.insert(Node::new("fresh", level, [Length::ZERO; 3]));
    assert!(fresh > model.max_id().unwrap());
    assert_eq!(loaded.nodes.len(), count + 1);
    // A well-formed document loads unchanged.
    assert_eq!(from_json(&to_json(&model)).unwrap(), model);

    // The same id in two tables is refused, by the loader and by compilation.
    let mut colliding = model.clone();
    let node = *colliding.nodes.keys().next().unwrap();
    colliding.groups.insert(
        node,
        Group {
            name: "colliding".into(),
            members: Default::default(),
        },
    );
    assert!(matches!(
        from_json(&to_json(&colliding)),
        Err(oa_model::format::FormatError::Corrupt(_))
    ));
    let problems = compile(&colliding).unwrap_err();
    assert!(
        problems
            .iter()
            .any(|p| p.entity == Some(node) && p.message.contains("more than one"))
    );

    // A group pointing at a missing entity is refused.
    let mut dangling = model.clone();
    let group = *dangling.groups.keys().next().unwrap();
    dangling
        .groups
        .get_mut(&group)
        .unwrap()
        .members
        .insert(EntityId(9_999));
    assert!(matches!(
        from_json(&to_json(&dangling)),
        Err(oa_model::format::FormatError::Corrupt(_))
    ));

    // An exhausted id space is refused rather than wrapped later.
    let mut exhausted = model.clone();
    exhausted.next_id = u64::MAX;
    assert!(matches!(
        from_json(&to_json(&exhausted)),
        Err(oa_model::format::FormatError::Corrupt(_))
    ));
    // And the allocator itself saturates instead of overflowing.
    let mut saturated = Model {
        next_id: u64::MAX,
        ..Default::default()
    };
    assert_eq!(saturated.allocate(), EntityId(u64::MAX));
    assert_eq!(saturated.allocate(), EntityId(u64::MAX));
}

#[test]
fn compile_reports_element_geometry_problems_by_entity() {
    let mut m = Model::default();
    let level = m.base_level().unwrap();
    let mat = m.insert(steel());
    let n0 = m.insert(Node::fixed("N0", level, [Length::ZERO; 3]));
    let shell = m.insert(Shell {
        name: "collapsed".into(),
        nodes: [n0; 4],
        material: mat,
        thickness: Length::from_si(0.1),
        formulation: ShellFormulation::Dkmq,
        drilling_ratio: 1e-3,
    });
    m.insert(LoadCase::new("empty"));
    let problems = compile(&m).unwrap_err();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].entity, Some(shell));
    assert_eq!(problems[0].name.as_deref(), Some("collapsed"));
    assert!(
        problems[0].message.contains("degenerate"),
        "{}",
        problems[0]
    );

    let mut m = Model::default();
    let level = m.base_level().unwrap();
    let mat = m.insert(steel());
    let sec = m.insert(section());
    let n0 = m.insert(Node::fixed("N0", level, [Length::ZERO; 3]));
    let n1 = m.insert(Node::new(
        "N1",
        level,
        [Length::from_si(3.0), Length::ZERO, Length::ZERO],
    ));
    let mut frame = Frame::new("twisted", [n0, n1], mat, sec);
    frame.local_y = Some([1.0, 0.0, 0.0]);
    let frame = m.insert(frame);
    m.insert(LoadCase::new("empty"));
    let problems = compile(&m).unwrap_err();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].entity, Some(frame));
    assert!(problems[0].message.contains("local_y"), "{}", problems[0]);
}

#[test]
fn save_json_preserves_the_previous_document_when_writing_fails() {
    let dir = std::env::temp_dir().join(format!("oa-model-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("model.json");
    let mut first = Model::default();
    first.metadata.name = "first".into();
    save_json(&first, &path).unwrap();
    assert_eq!(
        from_json(&std::fs::read_to_string(&path).unwrap()).unwrap(),
        first
    );

    // Block the temporary file: a directory sits where it would be created.
    let temporary = oa_model::format::temporary_path(&path);
    std::fs::create_dir(&temporary).unwrap();
    let mut second = first.clone();
    second.metadata.name = "second".into();
    assert!(save_json(&second, &path).is_err());
    assert_eq!(
        from_json(&std::fs::read_to_string(&path).unwrap()).unwrap(),
        first,
        "the previous document survives a failed save"
    );
    std::fs::remove_dir(&temporary).unwrap();

    save_json(&second, &path).unwrap();
    assert_eq!(
        from_json(&std::fs::read_to_string(&path).unwrap()).unwrap(),
        second
    );
    assert!(!temporary.exists(), "no temporary file is left behind");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn attach_refuses_an_unfinished_run() {
    let mut editor = Editor::new(Model::default());
    portal(&mut editor);
    // Add a load case that cannot be carried: a torque on a node whose only
    // member releases torsion at both ends. The run stops there.
    let m = &mut editor.model;
    let mat = m.find::<Material>("steel").unwrap();
    let sec = m.find::<Section>("column").unwrap();
    let level = m.base_level().unwrap();
    let base = m.insert(Node::fixed(
        "T0",
        level,
        [Length::from_si(20.0), Length::ZERO, Length::ZERO],
    ));
    let tip = m.insert(Node::new(
        "T1",
        level,
        [Length::from_si(23.0), Length::ZERO, Length::ZERO],
    ));
    let mut released = Frame::new("released", [base, tip], mat, sec);
    released.releases[3] = true;
    released.releases[9] = true;
    m.insert(released);
    let mut torque = LoadCase::new("torque");
    torque.nodal.push(NodalLoad {
        node: tip,
        force: [Force::ZERO; 3],
        moment: [Moment::from_si(100.0), Moment::ZERO, Moment::ZERO],
    });
    let torque = m.insert(torque);
    m.insert(Combination {
        name: "1.0T".into(),
        terms: vec![(torque, 1.0)],
    });
    let compiled = compile(m).unwrap();
    let dir = std::env::temp_dir().join(format!("oa-model-partial-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.sqlite");
    let options = oa_core::StaticOptions::default();
    let mut store = oa_results::ResultStore::create(&path, &compiled.solver, &options).unwrap();
    oa_core::analyze_static_into(&compiled.solver, &options, &mut store).unwrap_err();
    assert!(matches!(
        oa_model::store::attach(&compiled, &store),
        Err(oa_model::store::StoreError::IncompleteResults { .. })
    ));
    drop(store);
    assert!(oa_results::ResultStore::open(&path).is_err());
    let partial = oa_results::ResultStore::open_partial(&path).unwrap();
    assert!(matches!(
        oa_model::store::attach(&compiled, &partial),
        Err(oa_model::store::StoreError::IncompleteResults { missing }) if missing == ["1.0T"]
    ));
    drop(partial);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Levels: the 0/12/24 ft example from docs/model/LEVEL_SYSTEMS.md, driven
/// through commands with exact undo and redo.
#[test]
fn levels_move_bound_nodes_by_scope_and_undo_exactly() {
    let ft = Length::from_feet;
    let mut editor = Editor::new(Model::default());
    let base = editor.model.base_level().unwrap();
    let l1 = editor.model.allocate();
    let roof = editor.model.allocate();
    editor
        .apply(Command::AddLevel {
            id: l1,
            level: Level::new("L1", ft(12.0)),
        })
        .unwrap();
    editor
        .apply(Command::AddLevel {
            id: roof,
            level: Level::new("Roof", ft(24.0)),
        })
        .unwrap();
    // A node half a foot below L1, bound to it, and another at the same
    // height bound to Base: only the binding decides who follows a move.
    let bound = editor.model.allocate();
    let unbound = editor.model.allocate();
    let top = editor.model.allocate();
    for (id, name, level) in [(bound, "bound", l1), (unbound, "base-bound", base), (top, "top", roof)] {
        let z = if id == top { 24.0 } else { 11.5 };
        editor
            .apply(Command::AddNode {
                id,
                node: Node::new(name, level, [Length::ZERO, Length::ZERO, ft(z)]),
            })
            .unwrap();
    }
    let start = editor.model.clone();
    let z = |editor: &Editor, id: EntityId| editor.model.nodes[&id].position[2].si();
    let e = |editor: &Editor, id: EntityId| editor.model.levels[&id].elevation.si();

    editor
        .apply(Command::SetLevelElevation {
            id: l1,
            elevation: ft(14.0),
            scope: ElevationScope::ThisLevel,
        })
        .unwrap();
    assert!((e(&editor, l1) - ft(14.0).si()).abs() < 1e-12);
    assert!((e(&editor, roof) - ft(24.0).si()).abs() < 1e-12);
    assert!((z(&editor, bound) - ft(13.5).si()).abs() < 1e-12);
    assert!((z(&editor, unbound) - ft(11.5).si()).abs() < 1e-12);
    assert!((z(&editor, top) - ft(24.0).si()).abs() < 1e-12);
    assert!(editor.undo().unwrap());
    assert_eq!(editor.model, start, "undo restores the stored values exactly");

    editor
        .apply(Command::SetLevelElevation {
            id: l1,
            elevation: ft(14.0),
            scope: ElevationScope::ThisAndAbove,
        })
        .unwrap();
    assert!((e(&editor, roof) - ft(26.0).si()).abs() < 1e-12);
    assert!((z(&editor, top) - ft(26.0).si()).abs() < 1e-12);
    assert!((z(&editor, bound) - ft(13.5).si()).abs() < 1e-12);
    assert!((z(&editor, unbound) - ft(11.5).si()).abs() < 1e-12);
    let moved = editor.model.clone();
    assert!(editor.undo().unwrap());
    assert_eq!(editor.model, start);
    assert!(editor.redo().unwrap());
    assert_eq!(editor.model, moved);

    // Crossing or landing on a fixed level is refused and leaves no trace.
    let before = editor.model.clone();
    assert!(matches!(
        editor.apply(Command::SetLevelElevation {
            id: l1,
            elevation: ft(26.0),
            scope: ElevationScope::ThisLevel,
        }),
        Err(ModelError::Invalid(_))
    ));
    assert_eq!(editor.model, before);
    let [a, b, c, d, e] = <[_; 5]>::try_from((0..5).map(|_| editor.model.allocate()).collect::<Vec<_>>()).unwrap();
    assert!(matches!(
        editor.apply(Command::AddLevel {
            id: a,
            level: Level::new("dup", ft(14.0)),
        }),
        Err(ModelError::Invalid(_))
    ));
    assert!(matches!(
        editor.apply(Command::AddLevel {
            id: b,
            level: Level::new("", ft(50.0)),
        }),
        Err(ModelError::Invalid(_))
    ));
    assert!(matches!(
        editor.apply(Command::AddLevel {
            id: c,
            level: Level::new("L1", ft(50.0)),
        }),
        Err(ModelError::DuplicateName { .. })
    ));
    // A node cannot bind to something that is not a level.
    assert!(matches!(
        editor.apply(Command::AddNode {
            id: d,
            node: Node::new("stray", bound, [Length::ZERO; 3]),
        }),
        Err(ModelError::WrongKind { .. })
    ));
    assert!(matches!(
        editor.apply(Command::AddNode {
            id: e,
            node: Node::new("stray", EntityId(4_242), [Length::ZERO; 3]),
        }),
        Err(ModelError::Dangling { .. })
    ));
}

#[test]
fn level_moves_reject_newly_invalid_geometry_but_tolerate_old_problems() {
    let mut editor = Editor::new(Model::default());
    portal(&mut editor);
    // A point load 3.5 m up a 4 m column. Lowering L1 to 2 m would leave
    // the station past the end of the member: refused, and the load is
    // neither moved nor scaled.
    let column = editor.model.find::<Frame>("C1").unwrap();
    let case = editor.model.find::<LoadCase>("wind").unwrap();
    let mut load_case = editor.model.load_cases[&case].clone();
    load_case.member.push(MemberLoad::Point {
        member: column,
        position: Length::from_si(3.5),
        force: [Force::from_si(1_000.0), Force::ZERO, Force::ZERO],
        moment: [Moment::ZERO; 3],
        axes: Axes::Global,
    });
    editor
        .apply(Command::UpdateLoadCase { id: case, load_case })
        .unwrap();
    assert!(compile(&editor.model).is_ok());
    let l1 = editor.model.find::<Level>("L1").unwrap();
    let before = editor.model.clone();
    let err = editor
        .apply(Command::SetLevelElevation {
            id: l1,
            elevation: Length::from_si(2.0),
            scope: ElevationScope::ThisAndAbove,
        })
        .unwrap_err();
    assert!(err.to_string().contains("invalid"), "{err}");
    assert_eq!(editor.model, before);
    // Raising it keeps every station inside its member.
    editor
        .apply(Command::SetLevelElevation {
            id: l1,
            elevation: Length::from_si(4.5),
            scope: ElevationScope::ThisAndAbove,
        })
        .unwrap();
    assert!(editor.undo().unwrap());
    // An unrelated problem does not mask the check: with a zero modulus the
    // model fails to compile both before and after, and the move is still
    // refused for the station it would strand.
    let steel_id = editor.model.find::<Material>("steel").unwrap();
    let mut soft = editor.model.materials[&steel_id].clone();
    soft.young = Pressure::ZERO;
    editor
        .apply(Command::UpdateMaterial {
            id: steel_id,
            material: soft,
        })
        .unwrap();
    assert!(compile(&editor.model).is_err());
    let masked = editor.model.clone();
    let err = editor
        .apply(Command::SetLevelElevation {
            id: l1,
            elevation: Length::from_si(2.0),
            scope: ElevationScope::ThisAndAbove,
        })
        .unwrap_err();
    assert!(err.to_string().contains("outside its length"), "{err}");
    assert_eq!(editor.model, masked);
    assert!(editor.undo().unwrap());
    // A model that already has a problem can still have its levels edited.
    let dangling = Frame::new(
        "dangling",
        [editor.model.find::<Node>("N0").unwrap(), EntityId(9_999)],
        editor.model.find::<Material>("steel").unwrap(),
        editor.model.find::<Section>("column").unwrap(),
    );
    editor.model.insert(dangling);
    assert!(compile(&editor.model).is_err());
    editor
        .apply(Command::SetLevelElevation {
            id: l1,
            elevation: Length::from_si(5.0),
            scope: ElevationScope::ThisAndAbove,
        })
        .unwrap();
    assert!((editor.model.levels[&l1].elevation.si() - 5.0).abs() < 1e-12);
}

#[test]
fn level_removal_refuses_bound_nodes_and_the_last_level() {
    let mut editor = Editor::new(Model::default());
    let base = editor.model.base_level().unwrap();
    assert!(matches!(
        editor.apply(Command::RemoveLevel { id: base }),
        Err(ModelError::Invalid(_))
    ));
    let (_, _) = portal(&mut editor);
    let l2 = editor.model.find::<Level>("L2").unwrap();
    let err = editor.apply(Command::RemoveLevel { id: l2 }).unwrap_err();
    assert!(matches!(err, ModelError::Referenced { .. }), "{err}");
    // Rebinding the nodes first, keeping their coordinates, lets it go.
    let l1 = editor.model.find::<Level>("L1").unwrap();
    let commands = oa_model::levels::plan_remove(&editor.model, l2, l1).unwrap();
    let roof = editor.model.find::<Node>("N4").unwrap();
    editor.apply(Command::Batch { commands }).unwrap();
    assert!(!editor.model.levels.contains_key(&l2));
    assert_eq!(editor.model.nodes[&roof].level, l1);
    assert!((editor.model.nodes[&roof].position[2].si() - 8.0).abs() < 1e-12);
    assert!((oa_model::levels::offset(&editor.model.nodes[&roof], &editor.model.levels[&l1]) - 4.0).abs() < 1e-12);
    assert!(editor.undo().unwrap());
    assert!(editor.model.levels.contains_key(&l2));
    assert_eq!(editor.model.nodes[&roof].level, l2);
    // Ids are never reused, through deletion and undo alike.
    let fresh = editor.model.allocate();
    assert!(fresh > editor.model.max_id().unwrap());
}

/// An underlay is reference geometry on a level: it goes in and out through
/// commands, holds its level in place, survives a save, and never reaches
/// the solver.
#[test]
fn underlay_binds_to_a_level_and_stays_out_of_the_solver() {
    let mut editor = Editor::new(Model::default());
    portal(&mut editor);
    let hash =compile(&editor.model).unwrap().content_hash();
    let level = editor.model.find::<Level>("L2").unwrap();
    let point = |x: f64, y: f64| [Length::from_si(x), Length::from_si(y)];
    let underlay = Underlay {
        name: "roof plan".into(),
        level,
        origin: point(1.0, 2.0),
        segments: vec![[point(0.0, 0.0), point(6.0, 0.0)]],
    };
    let id = editor.model.allocate();
    editor
        .apply(Command::AddUnderlay {
            id,
            underlay: underlay.clone(),
        })
        .unwrap();
    assert_eq!(editor.model.kind_of(id), Some(EntityKind::Underlay));
    assert_eq!(compile(&editor.model).unwrap().content_hash(), hash);
    assert_eq!(from_json(&to_json(&editor.model)).unwrap(), editor.model);

    // A level cannot leave while a drawing lies on it, nodes or no nodes.
    let empty = editor.model.allocate();
    editor
        .apply(Command::AddLevel {
            id: empty,
            level: Level::new("L3", Length::from_si(12.0)),
        })
        .unwrap();
    editor
        .apply(Command::UpdateUnderlay {
            id,
            underlay: Underlay {
                level: empty,
                ..underlay.clone()
            },
        })
        .unwrap();
    assert!(matches!(
        editor.apply(Command::RemoveLevel { id: empty }),
        Err(ModelError::Referenced { .. })
    ));
    assert!(editor.undo().unwrap());
    assert_eq!(editor.model.underlays[&id], underlay);

    let bad = Underlay {
        name: "bad".into(),
        segments: vec![[point(f64::NAN, 0.0), point(1.0, 0.0)]],
        ..underlay.clone()
    };
    let bad_id = editor.model.allocate();
    assert!(matches!(
        editor.apply(Command::AddUnderlay {
            id: bad_id,
            underlay: bad
        }),
        Err(ModelError::Invalid(_))
    ));
    let missing = Underlay {
        name: "lost".into(),
        level: EntityId(u64::MAX - 1),
        ..underlay
    };
    assert!(matches!(
        editor.apply(Command::AddUnderlay {
            id: bad_id,
            underlay: missing
        }),
        Err(ModelError::Dangling { .. })
    ));

    editor.apply(Command::RemoveUnderlay { id }).unwrap();
    assert!(editor.model.underlays.is_empty());
    assert!(editor.undo().unwrap());
    assert_eq!(editor.model.underlays.len(), 1);
}

#[test]
fn format_v3_fixture_loads_with_its_underlay() {
    let model = from_json(include_str!("fixtures/format_v3.json")).unwrap();
    let underlay = &model.underlays[&EntityId(10)];
    assert_eq!(underlay.level, model.base_level().unwrap());
    assert_eq!(underlay.origin[1].si(), 2.0);
    assert_eq!(underlay.segments.len(), 2);
    assert_eq!(from_json(&to_json(&model)).unwrap(), model);
    // Without its underlays the document is the migrated version 2 fixture.
    let mut bare = model.clone();
    bare.underlays.clear();
    bare.next_id = 10;
    assert_eq!(
        bare,
        from_json(include_str!("fixtures/format_v2.json")).unwrap()
    );
    let mut adrift: serde_json::Value = serde_json::from_str(&to_json(&model)).unwrap();
    adrift["underlays"]["10"]["level"] = serde_json::json!(999);
    assert!(matches!(
        from_json(&adrift.to_string()),
        Err(oa_model::format::FormatError::Corrupt(_))
    ));
}
