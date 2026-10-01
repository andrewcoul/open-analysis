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
        local_x: None,
        modifiers: ShellModifiers::default(),
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

#[test]
fn format_v4_fixture_loads_with_a_section_shape() {
    let model = from_json(include_str!("fixtures/format_v4.json")).unwrap();
    let shape = model.sections[&EntityId(4)].shape.as_ref().unwrap();
    assert_eq!(shape.kind, ShapeKind::W);
    assert_eq!(shape.properties[&SectionProperty::Zx], 0.00257);
    assert_eq!(shape.properties[&SectionProperty::FlangeSlenderness], 10.2);
    assert_eq!(from_json(&to_json(&model)).unwrap(), model);
    // Without its shape the document is the version 3 fixture, and a
    // section without one is written as before.
    let mut bare = model.clone();
    bare.sections.get_mut(&EntityId(4)).unwrap().shape = None;
    assert_eq!(
        bare,
        from_json(include_str!("fixtures/format_v3.json")).unwrap()
    );
    assert!(!to_json(&bare).contains("shape"));
}

#[test]
fn aisc_library_copies_design_properties_in_si() {
    let library = Library::aisc();
    assert_eq!(library.sections.len(), 1523);
    assert!(library.materials.is_empty());
    for e in &library.sections {
        let shape = e.shape.as_ref();
        assert!(shape.is_some(), "{} has no shape", e.designation);
        assert!(
            [e.area, e.iy, e.iz, e.torsion].iter().all(|v| *v > 0.0),
            "{} needs positive stiffness properties",
            e.designation
        );
    }
    assert!(library.section_entry("L4X4X1/2").is_none());
    assert!(library.section_entry("2L4X4X1/2").is_none());

    // The model's spelling finds AISC's, and the copy records AISC's.
    let s = library.section("W14x90", "column").unwrap();
    let p = s.provenance.as_ref().unwrap();
    assert_eq!(p.library, "aisc-shapes");
    assert_eq!(p.version, "16.0");
    assert_eq!(p.designation, "W14X90");
    let inch = 0.0254_f64;
    assert!((s.area.si() - 26.5 * inch.powi(2)).abs() < 1e-12);
    assert!((s.iz.si() - 999.0 * inch.powi(4)).abs() < 1e-12);
    let shape = s.shape.as_ref().unwrap();
    assert_eq!(shape.kind, ShapeKind::W);
    let value = |p: SectionProperty| shape.properties[&p];
    assert!((value(SectionProperty::Depth) - 14.0 * inch).abs() < 1e-12);
    assert!((value(SectionProperty::Zx) - 157.0 * inch.powi(3)).abs() < 1e-12);
    assert!((value(SectionProperty::Cw) - 16_000.0 * inch.powi(6)).abs() < 1e-15);
    assert_eq!(value(SectionProperty::FlangeSlenderness), 10.2);

    // Shown in US units, every property reads back as tabulated.
    let shown = UnitSystem::UsCustomary.display(&s);
    let table = &library.section_entry("W14X90").unwrap().shape;
    for (property, tabulated) in &table.as_ref().unwrap().properties {
        let back = shown.shape.as_ref().unwrap().properties[property];
        assert!((back - tabulated).abs() <= 1e-9 * tabulated.abs(), "{property:?}");
    }

    // A round HSS has a diameter and no warping constant.
    let round = library.section("HSS8.625X0.500", "brace").unwrap();
    let round = round.shape.unwrap();
    assert_eq!(round.kind, ShapeKind::HSS);
    assert!(round.properties.contains_key(&SectionProperty::OutsideDiameter));
    assert!(!round.properties.contains_key(&SectionProperty::Cw));
}

#[test]
fn format_v5_fixture_loads_with_material_strengths() {
    let model = from_json(include_str!("fixtures/format_v5.json")).unwrap();
    let steel = &model.materials[&EntityId(3)];
    assert_eq!(steel.fy, Some(Pressure::from_si(345e6)));
    assert_eq!(steel.fu, Some(Pressure::from_si(450e6)));
    assert_eq!(steel.fc, None);
    assert_eq!(from_json(&to_json(&model)).unwrap(), model);
    // Without strengths the document is the version 4 fixture, and a
    // material without them is written as before.
    let mut bare = model.clone();
    let m = bare.materials.get_mut(&EntityId(3)).unwrap();
    (m.fy, m.fu) = (None, None);
    assert_eq!(
        bare,
        from_json(include_str!("fixtures/format_v4.json")).unwrap()
    );
    let json = to_json(&bare);
    assert!(!json.contains("\"fy\"") && !json.contains("\"fu\"") && !json.contains("\"fc\""));
}

#[test]
fn starter_library_materials_carry_strengths_in_si() {
    let library = Library::starter();
    let ksi = 6.894_757_293_168e6;
    let steel = library.material("A992", "steel").unwrap();
    assert!((steel.fy.unwrap().si() - 50.0 * ksi).abs() < 1e-3);
    assert!((steel.fu.unwrap().si() - 65.0 * ksi).abs() < 1e-3);
    assert_eq!(steel.fc, None);
    let round = library.material("A500 Gr. C round", "tube").unwrap();
    let rectangular = library.material("A500 Gr. C rectangular", "tube").unwrap();
    assert!(round.fy.unwrap().si() < rectangular.fy.unwrap().si());
    let concrete = library.material("Concrete 4 ksi", "slab").unwrap();
    assert!((concrete.fc.unwrap().si() - 4.0 * ksi).abs() < 1e-3);
    assert_eq!((concrete.fy, concrete.fu), (None, None));
    // Every steel entry has both strengths, every concrete one f'c.
    for e in &library.materials {
        let concrete = e.designation.starts_with("Concrete");
        assert_eq!(e.fc.is_some(), concrete, "{}", e.designation);
        assert_eq!(
            e.fy.is_some() && e.fu.is_some(),
            !concrete,
            "{}",
            e.designation
        );
    }
    // Shown in US units, the strengths read back as tabulated.
    let shown = UnitSystem::UsCustomary.display(&steel);
    assert!((shown.fy.unwrap().si() - 50.0).abs() < 1e-9);
    assert!((shown.fu.unwrap().si() - 65.0).abs() < 1e-9);
}

#[test]
fn material_strengths_are_validated_and_undone() {
    let mut editor = Editor::new(Model::default());
    let id = editor.model.allocate();
    let invalid = |material: Material| {
        let mut probe = editor.model.clone();
        Command::AddMaterial { id, material }
            .apply(&mut probe)
            .unwrap_err()
            .to_string()
    };
    let ksi = |v: f64| Some(Pressure::from_si(v * 6.894_757_293_168e6));
    for (fy, fu, fc) in [
        (ksi(0.0), None, None),
        (ksi(-36.0), None, None),
        (None, Some(Pressure::from_si(f64::NAN)), None),
        (None, None, ksi(-4.0)),
    ] {
        let err = invalid(Material {
            fy,
            fu,
            fc,
            ..steel()
        });
        assert!(err.contains("must be positive and finite"), "{err}");
    }
    let err = invalid(Material {
        fy: ksi(65.0),
        fu: ksi(50.0),
        ..steel()
    });
    assert!(err.contains("Fu must be at least Fy"), "{err}");
    assert!(editor.model.materials.is_empty());

    // A material with no strengths is still valid, and an update that
    // changes them is undone like any other.
    let bare = Material {
        fy: None,
        fu: None,
        ..steel()
    };
    editor
        .apply(Command::AddMaterial { id, material: bare })
        .unwrap();
    let graded = Material {
        fy: ksi(50.0),
        fu: ksi(65.0),
        ..steel()
    };
    editor
        .apply(Command::UpdateMaterial {
            id,
            material: graded.clone(),
        })
        .unwrap();
    assert_eq!(editor.model.materials[&id], graded);
    let err = editor
        .apply(Command::UpdateMaterial {
            id,
            material: Material {
                fu: ksi(40.0),
                ..graded.clone()
            },
        })
        .unwrap_err();
    assert!(err.to_string().contains("Fu must be at least Fy"), "{err}");
    assert!(editor.undo().unwrap());
    assert_eq!(editor.model.materials[&id].fy, None);
}

#[test]
fn format_v6_fixture_loads_with_shear_areas() {
    let model = from_json(include_str!("fixtures/format_v6.json")).unwrap();
    let beam = &model.sections[&EntityId(4)];
    assert_eq!(beam.shear_y, Some(Area::from_si(0.00397)));
    assert_eq!(beam.shear_z, Some(Area::from_si(0.011)));
    assert_eq!(from_json(&to_json(&model)).unwrap(), model);
    // They reach the solver, and without them the document is the version
    // 5 fixture, written as before.
    let solver = &compile(&model).unwrap().solver.sections[0];
    assert_eq!(
        (solver.shear_y, solver.shear_z),
        (beam.shear_y, beam.shear_z)
    );
    let mut bare = model.clone();
    let s = bare.sections.get_mut(&EntityId(4)).unwrap();
    (s.shear_y, s.shear_z) = (None, None);
    assert_eq!(
        bare,
        from_json(include_str!("fixtures/format_v5.json")).unwrap()
    );
    assert!(!to_json(&bare).contains("shear_"));
}

#[test]
fn aisc_library_copies_shear_areas() {
    let library = Library::aisc();
    let in2 = |v: f64| Area::from_square_inches(v).si();
    let shear = |designation: &str| {
        let s = library.section(designation, "s").unwrap();
        (
            s.shear_y.unwrap().si(),
            s.shear_z.unwrap().si(),
            s.area.si(),
        )
    };
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-12 * b;
    // W14X90: d tw along the web, 5/6 of both flanges across them.
    let (y, z, _) = shear("W14X90");
    assert!(near(y, in2(14.0 * 0.44)) && near(z, in2(5.0 / 3.0 * 14.5 * 0.71)));
    // A tee has one flange; its stem lies along local y as its web would.
    let (y, z, _) = shear("WT7X45");
    assert!(near(y, in2(7.01 * 0.44)) && near(z, in2(5.0 / 6.0 * 14.5 * 0.71)));
    // A rectangular tube's two walls of height Ht carry shear along y.
    let (y, z, _) = shear("HSS12X8X1/2");
    assert!(near(y, in2(2.0 * 12.0 * 0.465)) && near(z, in2(2.0 * 8.0 * 0.465)));
    // A round tube or pipe carries (0.5 + 0.8 t / OD) A either way, CSI's
    // (0.9 - 0.4 s) A with s = (r - t) / r: near half for a thin wall, more
    // for a thick one. Pipe2XXS (A 2.51, OD 2.375, tdes 0.406) is 1.598 in².
    for (round, expected) in [
        ("HSS8.625X0.500", (0.5 + 0.8 * 0.465 / 8.63) * 11.9),
        ("Pipe2XXS", (0.5 + 0.8 * 0.406 / 2.375) * 2.51),
    ] {
        let (y, z, _) = shear(round);
        assert!(near(y, in2(expected)) && near(z, y), "{round}");
    }
    let (y, _, area) = shear("Pipe2XXS");
    assert!((y / in2(1.0) - 1.5983).abs() < 1e-4 && y > area / 2.0 * 1.27);
    // Every shape gets both, smaller than its area; the starter library's
    // sections carry no shape and stay rigid in shear.
    for e in &library.sections {
        let (y, z, area) = shear(&e.designation);
        assert!(y > 0.0 && z > 0.0, "{}", e.designation);
        assert!(y < area && z < area, "{}", e.designation);
    }
    assert_eq!(section().shear_y, None);
    // Shown in US units they read in in².
    let shown = UnitSystem::UsCustomary.display(&library.section("W14X90", "s").unwrap());
    assert!((shown.shear_y.unwrap().si() - 6.16).abs() < 1e-9);
}

#[test]
fn shear_areas_are_validated_and_undone() {
    let mut editor = Editor::new(Model::default());
    let id = editor.model.allocate();
    for bad in [0.0, -1.0, f64::NAN] {
        let mut probe = editor.model.clone();
        let err = Command::AddSection {
            id,
            section: Section {
                shear_z: Some(Area::from_si(bad)),
                ..section()
            },
        }
        .apply(&mut probe)
        .unwrap_err();
        assert!(
            err.to_string().contains("shear areas must be positive"),
            "{err}"
        );
    }
    editor
        .apply(Command::AddSection {
            id,
            section: section(),
        })
        .unwrap();
    let sheared = Section {
        shear_y: Some(Area::from_square_inches(6.16)),
        ..section()
    };
    editor
        .apply(Command::UpdateSection {
            id,
            section: sheared.clone(),
        })
        .unwrap();
    assert_eq!(editor.model.sections[&id], sheared);
    assert!(editor.undo().unwrap());
    assert_eq!(editor.model.sections[&id].shear_y, None);
}

#[test]
fn format_v7_fixture_loads_with_stiffness_modifiers() {
    let model = from_json(include_str!("fixtures/format_v7.json")).unwrap();
    let beam = &model.frames[&EntityId(5)];
    let cracked = FrameModifiers {
        iy: 0.35,
        iz: 0.35,
        ..Default::default()
    };
    assert_eq!(beam.modifiers, cracked);
    assert_eq!(from_json(&to_json(&model)).unwrap(), model);
    // They reach the solver, and without them the document is the version
    // 6 fixture, written as before.
    assert_eq!(compile(&model).unwrap().solver.frames[0].modifiers, cracked);
    let mut bare = model.clone();
    bare.frames.get_mut(&EntityId(5)).unwrap().modifiers = FrameModifiers::default();
    assert_eq!(
        bare,
        from_json(include_str!("fixtures/format_v6.json")).unwrap()
    );
    assert!(!to_json(&bare).contains("modifiers"));
}

#[test]
fn stiffness_modifiers_are_validated_and_undone() {
    let mut editor = Editor::new(Model::default());
    portal(&mut editor);
    let (&frame, _) = editor
        .model
        .frames
        .iter()
        .find(|(_, f)| f.name == "C1")
        .unwrap();
    let mut shell_probe = editor.model.clone();
    let shell_id = shell_probe.allocate();
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut probe = editor.model.clone();
        let mut edited = probe.frames[&frame].clone();
        edited.modifiers.torsion = bad;
        let err = Command::UpdateFrame {
            id: frame,
            frame: edited,
        }
        .apply(&mut probe)
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("stiffness modifiers must be positive"),
            "{err}"
        );
        let err = Command::AddShell {
            id: shell_id,
            shell: Shell {
                name: "wall".into(),
                nodes: [frame; 4],
                material: frame,
                thickness: Length::from_si(0.2),
                formulation: ShellFormulation::Dkmq,
                drilling_ratio: 1e-3,
                local_x: None,
                modifiers: ShellModifiers {
                    bending: bad,
                    ..Default::default()
                },
            },
        }
        .apply(&mut shell_probe)
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("stiffness modifiers must be positive"),
            "{err}"
        );
    }
    let mut column = editor.model.frames[&frame].clone();
    column.modifiers.iy = 0.7;
    column.modifiers.iz = 0.7;
    editor
        .apply(Command::UpdateFrame {
            id: frame,
            frame: column.clone(),
        })
        .unwrap();
    assert_eq!(editor.model.frames[&frame], column);
    assert!(editor.undo().unwrap());
    assert!(editor.model.frames[&frame].modifiers.is_unmodified());
}

#[test]
fn shell_local_axes_and_modifiers_reach_the_solver() {
    let mut m = Model::default();
    let level = m.base_level().unwrap();
    let mat = m.insert(steel());
    let nodes = [(0.0, 0.0), (4.0, 0.0), (4.0, 3.0), (0.0, 3.0)].map(|(x, z)| {
        let p = [Length::from_si(x), Length::ZERO, Length::from_si(z)];
        m.insert(Node::fixed(format!("N{x}-{z}"), level, p))
    });
    let wall = Shell {
        name: "W1".into(),
        nodes,
        material: mat,
        thickness: Length::from_si(0.2),
        formulation: ShellFormulation::Dkmq,
        drilling_ratio: 1e-3,
        local_x: Some([0.0, 0.0, 1.0]),
        modifiers: ShellModifiers {
            membrane_y: 0.35,
            ..Default::default()
        },
    };
    let id = m.insert(wall.clone());
    m.insert(LoadCase::new("empty"));
    assert_eq!(from_json(&to_json(&m)).unwrap(), m);
    let solver = &compile(&m).unwrap().solver.shells[0];
    assert_eq!(solver.local_x, wall.local_x);
    assert_eq!(solver.modifiers, wall.modifiers);

    // A local x normal to the wall is a compile problem on that shell.
    m.shells.get_mut(&id).unwrap().local_x = Some([0.0, 1.0, 0.0]);
    let problems = compile(&m).unwrap_err();
    assert_eq!(problems[0].entity, Some(id));
    assert!(
        problems[0].message.contains("local_x is normal"),
        "{}",
        problems[0]
    );
}

#[test]
fn format_v8_fixture_loads_with_mass_sources() {
    let model = from_json(include_str!("fixtures/format_v8.json")).unwrap();
    let seismic = model.find::<MassSource>("seismic").unwrap();
    let dead_only = model.find::<MassSource>("dead only").unwrap();
    assert_eq!(model.default_mass_source, Some(seismic));
    assert_eq!(
        model.mass_sources[&seismic].cases,
        vec![(EntityId(6), 1.0), (EntityId(11), 0.25)]
    );
    assert!(!model.mass_sources[&seismic].vertical);
    assert!(model.mass_sources[&dead_only].vertical);
    assert_eq!(model.frames[&EntityId(5)].modifiers.mass, 0.5);
    assert_eq!(from_json(&to_json(&model)).unwrap(), model);
    // The default reaches the solver with cases by table position. The tip
    // holds its own 100 kg, half of half the beam's mass, all of the
    // superimposed dead load and a quarter of the storage live load, each
    // 9806.65 N or 1000 kg, and no vertical mass.
    let compiled = compile(&model).unwrap();
    assert_eq!(
        compiled.solver.mass_source.cases,
        vec![
            (oa_core::LoadCaseId(0), 1.0),
            (oa_core::LoadCaseId(1), 0.25)
        ]
    );
    let own = 100.0 + 0.5 * 7850.0 * 0.01 * 3.0 / 2.0;
    let free = |solver: &oa_core::Model| {
        oa_core::analyze_modal(solver, &Default::default())
            .unwrap()
            .total_free_mass
    };
    let seismic_mass = free(&compiled.solver);
    assert!((seismic_mass[0] / (own + 1250.0) - 1.0).abs() < 1e-12);
    assert_eq!(seismic_mass[2], 0.0);
    // Another source is asked for by id.
    let dead = free(&compiled.with_mass_source(&model, dead_only).unwrap());
    assert!((dead[2] / (own + 1000.0) - 1.0).abs() < 1e-12);
    assert!(compiled.with_mass_source(&model, EntityId(5)).is_none());
    // A version 7 document has no sources and uses element and node mass.
    let older = from_json(include_str!("fixtures/format_v7.json")).unwrap();
    assert!(older.mass_sources.is_empty() && older.default_mass_source.is_none());
    assert!(!to_json(&older).contains("default_mass_source"));
    assert!(compile(&older).unwrap().solver.mass_source.is_default());
}

#[test]
fn mass_sources_are_validated_and_undone() {
    let mut editor = Editor::new(Model::default());
    portal(&mut editor);
    let frame = editor.model.find::<Frame>("B1").unwrap();
    let [sdl, dead, seismic, other] = [(); 4].map(|_| editor.model.allocate());
    editor
        .apply(Command::AddLoadCase {
            id: sdl,
            load_case: LoadCase::new("SDL").with_type(LoadType::Dead),
        })
        .unwrap();
    editor
        .apply(Command::AddLoadCase {
            id: dead,
            load_case: LoadCase {
                self_weight: [0.0, 0.0, -1.0],
                ..LoadCase::new("D")
            },
        })
        .unwrap();
    let source = |cases: Vec<(EntityId, f64)>| MassSource {
        cases,
        ..MassSource::new("seismic")
    };
    let add = |mass_source: MassSource| Command::AddMassSource {
        id: seismic,
        mass_source,
    };
    fn refused(model: &Model, command: Command) -> String {
        let mut probe = model.clone();
        command.apply(&mut probe).unwrap_err().to_string()
    }
    let m = &editor.model;
    let missing = EntityId(9_999);
    assert!(refused(m, add(source(vec![(missing, 1.0)]))).contains("missing load case"));
    assert!(refused(m, add(source(vec![(frame, 1.0)]))).contains("not a load case"));
    assert!(refused(m, add(source(vec![(sdl, 1.0), (sdl, 1.0)]))).contains("twice"));
    for bad in [0.0, -0.25, f64::NAN] {
        assert!(refused(m, add(source(vec![(sdl, bad)]))).contains("positive and finite"));
    }
    let neither = MassSource {
        lateral: false,
        vertical: false,
        ..source(vec![])
    };
    assert!(refused(m, add(neither)).contains("lateral or vertical"));

    let seismic_source = MassSource {
        element_mass: false,
        vertical: false,
        ..source(vec![(dead, 1.0), (sdl, 1.0)])
    };
    editor.apply(add(seismic_source.clone())).unwrap();
    assert_eq!(editor.model.mass_sources[&seismic], seismic_source);
    let err = refused(
        &editor.model,
        Command::AddMassSource {
            id: other,
            mass_source: MassSource::new("seismic"),
        },
    );
    assert!(err.contains("already used"), "{err}");
    editor
        .apply(Command::SetDefaultMassSource { id: Some(seismic) })
        .unwrap();
    // A case a source lists, and the default source, cannot be removed.
    let err = refused(&editor.model, Command::RemoveLoadCase { id: sdl });
    assert!(err.contains("mass source"), "{err}");
    let err = refused(&editor.model, Command::RemoveMassSource { id: seismic });
    assert!(err.contains("default"), "{err}");
    let err = refused(
        &editor.model,
        Command::SetDefaultMassSource { id: Some(frame) },
    );
    assert!(err.contains("not a mass source"), "{err}");
    let err = refused(
        &editor.model,
        Command::SetDefaultMassSource { id: Some(missing) },
    );
    assert!(err.contains("no entity"), "{err}");
    assert!(editor.undo().unwrap());
    assert_eq!(editor.model.default_mass_source, None);
    assert!(editor.undo().unwrap());
    assert!(editor.model.mass_sources.is_empty());
    assert!(editor.redo().unwrap());
    assert!(editor.redo().unwrap());
    assert_eq!(editor.model.default_mass_source, Some(seismic));

    // Self-weight added to a source case afterwards, with element mass on,
    // is caught when the model compiles, against that source.
    editor
        .apply(Command::UpdateMassSource {
            id: seismic,
            mass_source: source(vec![(sdl, 1.0)]),
        })
        .unwrap();
    let mut weighed = editor.model.load_cases[&sdl].clone();
    weighed.self_weight = [0.0, 0.0, -1.0];
    editor
        .apply(Command::UpdateLoadCase {
            id: sdl,
            load_case: weighed,
        })
        .unwrap();
    let problems = compile(&editor.model).unwrap_err();
    assert_eq!(problems[0].entity, Some(seismic));
    assert!(
        problems[0].message.contains("self-weight"),
        "{}",
        problems[0]
    );

    // That state is reachable, so every inverse out of it must apply:
    // fixing the source and undoing the fix, removing it and undoing the
    // removal, and a batch that removes it and then fails.
    let broken = editor.model.mass_sources[&seismic].clone();
    editor
        .apply(Command::UpdateMassSource {
            id: seismic,
            mass_source: MassSource {
                element_mass: false,
                ..broken.clone()
            },
        })
        .unwrap();
    assert!(compile(&editor.model).is_ok());
    assert!(editor.undo().unwrap());
    assert_eq!(editor.model.mass_sources[&seismic], broken);
    editor
        .apply(Command::SetDefaultMassSource { id: None })
        .unwrap();
    editor
        .apply(Command::RemoveMassSource { id: seismic })
        .unwrap();
    assert!(editor.undo().unwrap());
    assert_eq!(editor.model.mass_sources[&seismic], broken);
    let before = editor.model.clone();
    let err = Command::Batch {
        commands: vec![
            Command::RemoveMassSource { id: seismic },
            Command::RemoveLoadCase { id: missing },
        ],
    }
    .apply(&mut editor.model)
    .unwrap_err();
    assert!(matches!(err, ModelError::Batch { index: 1, .. }), "{err}");
    assert_eq!(editor.model, before);
}

/// A column up Z from Base through L1 at 4 m to a node 1 m above it, with
/// 40 kg at 1 m, 8 kg halfway at 2 m and 10 kg at the top.
fn lumped_column() -> (Model, [EntityId; 6]) {
    let mut m = Model::default();
    let base = m.base_level().unwrap();
    let l1 = m.insert(Level::new("L1", Length::from_si(4.0)));
    let mat = m.insert(steel());
    let sec = m.insert(section());
    let at = |x: f64, z: f64| [Length::from_si(x), Length::ZERO, Length::from_si(z)];
    let node = |m: &mut Model, name: &str, level, z: f64, mass: f64| {
        let mut n = Node::new(name, level, at(0.0, z));
        n.mass = [Mass::from_si(mass); 3];
        m.insert(n)
    };
    let n0 = m.insert(Node::fixed("N0", base, at(0.0, 0.0)));
    let low = node(&mut m, "low", base, 1.0, 40.0);
    let half = node(&mut m, "half", base, 2.0, 8.0);
    let n1 = node(&mut m, "N1", l1, 4.0, 0.0);
    let top = node(&mut m, "top", l1, 5.0, 10.0);
    for (name, a, b) in [
        ("C1", n0, low),
        ("C2", low, half),
        ("C3", half, n1),
        ("C4", n1, top),
    ] {
        m.insert(Frame::new(name, [a, b], mat, sec));
    }
    let source = m.insert(MassSource {
        element_mass: false,
        lump_to_levels: true,
        ..MassSource::new("lumped")
    });
    m.default_mass_source = Some(source);
    (m, [n0, low, half, n1, top, source])
}

#[test]
fn lumping_to_levels_moves_lateral_mass_to_the_nearest_level() {
    let (mut model, [n0, low, half, n1, top, source]) = lumped_column();
    let compiled = compile(&model).unwrap();
    let index = |id: EntityId| oa_core::NodeId(compiled.mapping.node_index[&id]);
    // Nearest Base, halfway between Base and L1, and above the highest level.
    assert_eq!(
        compiled.solver.mass_source.lump,
        vec![
            (index(low), vec![(index(n0), 1.0)]),
            (index(half), vec![(index(n0), 0.5), (index(n1), 0.5)]),
            (index(top), vec![(index(n1), 1.0)]),
        ]
    );
    // Lateral mass lumped onto the fixed base is no longer free; vertical
    // mass stays where it was.
    let free = oa_core::analyze_modal(&compiled.solver, &Default::default())
        .unwrap()
        .total_free_mass;
    assert!((free[0] - 14.0).abs() < 1e-9 && (free[2] - 58.0).abs() < 1e-9);
    // A node off a level with no node directly below it on the nearest one.
    let base = model.base_level().unwrap();
    let stray = model.insert(Node::new(
        "stray",
        base,
        [Length::from_si(3.0), Length::ZERO, Length::from_si(1.0)],
    ));
    let problems = compile(&model).unwrap_err();
    assert_eq!(problems[0].entity, Some(source));
    assert!(
        problems[0].message.contains(&model.describe(stray))
            && problems[0].message.contains("below it on level \"Base\""),
        "{}",
        problems[0]
    );
}

#[test]
fn format_v10_fixture_loads_with_grid_lines() {
    let model = from_json(include_str!("fixtures/format_v10.json")).unwrap();
    let b = model.find::<GridLine>("B").unwrap();
    assert_eq!(model.grid_lines.len(), 3);
    assert_eq!(model.grid_lines[&b].start[0].si(), 3.0);
    assert_eq!(model.kind_of(b), Some(EntityKind::GridLine));
    assert_eq!(from_json(&to_json(&model)).unwrap(), model);
    // Without its grid lines the document is the migrated version 9 fixture.
    let mut bare = model.clone();
    bare.grid_lines.clear();
    bare.next_id = 9;
    assert_eq!(
        bare,
        from_json(include_str!("fixtures/format_v9.json")).unwrap()
    );
}

/// Grid lines are model-wide reference geometry: they go in and out
/// through commands, a rectangular grid is one undo step, labels are
/// names, and the solver never sees them.
#[test]
fn grid_lines_are_reference_geometry_added_as_one_step() {
    let mut editor = Editor::new(Model::default());
    portal(&mut editor);
    let hash = compile(&editor.model).unwrap().content_hash();
    let ft = |v: f64| Length::from_si(v * 0.3048);
    let grid = grids::RectangularGrid {
        origin: [ft(0.0), ft(0.0)],
        x_spacings: vec![ft(30.0); 3],
        y_spacings: vec![ft(25.0); 2],
        x_label: "A".into(),
        y_label: "1".into(),
        overhang: ft(5.0),
    };
    let command = grid.command(&editor.model).unwrap();
    editor.apply(command).unwrap();
    assert_eq!(editor.model.grid_lines.len(), 7);
    let d = editor.model.find::<GridLine>("D").unwrap();
    let line = editor.model.grid_lines[&d].clone();
    assert!((line.start[0].si() - ft(90.0).si()).abs() < 1e-12);
    assert!((line.end[1].si() - ft(55.0).si()).abs() < 1e-12);
    assert_eq!(compile(&editor.model).unwrap().content_hash(), hash);
    assert_eq!(from_json(&to_json(&editor.model)).unwrap(), editor.model);

    // A second grid with the same labels is refused whole.
    let again = grid.command(&editor.model).unwrap();
    assert!(matches!(editor.apply(again), Err(ModelError::Batch { .. })));
    assert_eq!(editor.model.grid_lines.len(), 7);

    // Lines are edited and removed one at a time.
    let moved = GridLine {
        name: "D.5".into(),
        start: [ft(105.0), ft(-5.0)],
        ..line.clone()
    };
    editor
        .apply(Command::UpdateGridLine {
            id: d,
            grid_line: moved.clone(),
        })
        .unwrap();
    assert_eq!(editor.model.grid_lines[&d], moved);
    let point = GridLine {
        end: moved.start,
        ..moved.clone()
    };
    assert!(matches!(
        editor.apply(Command::UpdateGridLine {
            id: d,
            grid_line: point
        }),
        Err(ModelError::Invalid(_))
    ));
    let nan = GridLine {
        start: [Length::from_si(f64::NAN), ft(0.0)],
        ..moved
    };
    assert!(matches!(
        editor.apply(Command::UpdateGridLine {
            id: d,
            grid_line: nan
        }),
        Err(ModelError::Invalid(_))
    ));
    editor.apply(Command::RemoveGridLine { id: d }).unwrap();
    assert_eq!(editor.model.grid_lines.len(), 6);
    assert!(editor.undo().unwrap());
    assert!(editor.undo().unwrap());
    assert_eq!(editor.model.grid_lines[&d], line);
    // Undoing the grid takes every line out at once.
    assert!(editor.undo().unwrap());
    assert!(editor.model.grid_lines.is_empty());
}

#[test]
fn format_v9_fixture_loads_with_frame_offsets() {
    let model = from_json(include_str!("fixtures/format_v9.json")).unwrap();
    let b1 = model.find::<Frame>("B1").unwrap();
    let b2 = model.find::<Frame>("B2").unwrap();
    assert_eq!(model.frames[&b1].offsets.rigid_zone, 0.5);
    assert_eq!(model.frames[&b1].cardinal_point, CardinalPoint::Centroid);
    assert_eq!(model.frames[&b2].cardinal_point, CardinalPoint::TopCenter);
    assert_eq!(model.frames[&b2].cardinal_point.number(), 8);
    assert_eq!(from_json(&to_json(&model)).unwrap(), model);
    // The offsets reach the solver as they are; the cardinal point adds to
    // the joint offsets: the top of a 0.356 m beam on its nodes puts the
    // centroid 0.178 m below them.
    let compiled = compile(&model).unwrap();
    let [f1, f2] = [b1, b2].map(|id| &compiled.solver.frames[compiled.mapping.frame_index[&id]]);
    assert_eq!(f1.offsets, model.frames[&b1].offsets);
    for end in f2.offsets.joint {
        assert!((end[0].si()).abs() < 1e-15);
        assert!((end[1].si() - 0.1).abs() < 1e-15);
        assert!((end[2].si() + 0.178).abs() < 1e-12);
    }
    let results = oa_core::analyze_static(&compiled.solver, &Default::default()).unwrap();
    assert!(results.combinations[0].frames.is_some());
    // A version 8 frame has no offsets and writes none.
    let older = from_json(include_str!("fixtures/format_v8.json")).unwrap();
    assert!(older.frames.values().all(|f| f.offsets.is_none()));
    assert!(!to_json(&older).contains("offsets"));
    assert!(!to_json(&older).contains("cardinal_point"));
}

#[test]
fn level_moves_name_what_keeps_a_frame_from_resolving() {
    let mut editor = Editor::new(Model::default());
    portal(&mut editor);
    // A brace from N1 (6, 0, 0) up to N4 (0, 0, 8), hung by its top with
    // local_y [-1, 0, 1]. Lowering L2 to 6 m turns it parallel to local_y,
    // so its cardinal point has no axes to be read in.
    let shaped = editor.model.allocate();
    editor
        .apply(Command::AddSection {
            id: shaped,
            section: Library::aisc().section("W14x90", "shaped").unwrap(),
        })
        .unwrap();
    let steel = editor.model.find::<Material>("steel").unwrap();
    let [n1, n4] = ["N1", "N4"].map(|n| editor.model.find::<Node>(n).unwrap());
    let mut brace = Frame::new("D1", [n1, n4], steel, shaped);
    brace.local_y = Some([-1.0, 0.0, 1.0]);
    brace.cardinal_point = CardinalPoint::TopCenter;
    let id = editor.model.allocate();
    editor
        .apply(Command::AddFrame { id, frame: brace })
        .unwrap();
    assert!((frame_length(&editor.model, id).unwrap() - 10.0).abs() < 1e-12);
    let l2 = editor.model.find::<Level>("L2").unwrap();
    let err = editor
        .apply(Command::SetLevelElevation {
            id: l2,
            elevation: Length::from_si(6.0),
            scope: ElevationScope::ThisLevel,
        })
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("local_y is parallel") && !err.contains("zero or invalid length"),
        "{err}"
    );
}

#[test]
fn cardinal_points_sit_across_the_member_their_joint_offsets_leave() {
    // Lift B2's end J 3 m so the member slopes from (0, 0.1, 0) to
    // (6, 0.1, 3). The top of its section must sit on those two points,
    // measured across the sloped member, not the level line between nodes.
    let fixture = from_json(include_str!("fixtures/format_v9.json")).unwrap();
    let b2 = fixture.find::<Frame>("B2").unwrap();
    let in_global = [[0.0, 0.1, 0.0], [0.0, 0.1, 3.0]];
    // The same move in the axes of the level line: x along global X, y up,
    // and z toward global -Y.
    let in_local = [[0.0, 0.0, -0.1], [0.0, 3.0, -0.1]];
    for (axes, joint) in [
        (oa_core::Axes::Global, in_global),
        (oa_core::Axes::Local, in_local),
    ] {
        let mut model = fixture.clone();
        let frame = model.frames.get_mut(&b2).unwrap();
        frame.offsets.axes = axes;
        frame.offsets.joint = joint.map(|end| end.map(Length::from_si));
        let compiled = compile(&model).unwrap();
        let solver = &compiled.solver.frames[compiled.mapping.frame_index[&b2]];
        assert_eq!(solver.offsets.axes, oa_core::Axes::Global);
        let centroid = solver.ends([[0.0; 3], [6.0, 0.0, 0.0]]).unwrap();
        let span = std::array::from_fn(|i| centroid[1][i] - centroid[0][i]);
        let [_, up, _] = solver.axes_along(span).unwrap();
        for (k, expected) in [[0.0, 0.1, 0.0], [6.0, 0.1, 3.0]].into_iter().enumerate() {
            for i in 0..3 {
                let top = centroid[k][i] + up[i] * 0.356 / 2.0;
                assert!(
                    (top - expected[i]).abs() < 1e-12,
                    "{axes:?} end {k}: {:?}",
                    centroid[k]
                );
            }
        }
    }
}

#[test]
fn cardinal_points_follow_the_section_shape() {
    let library = Library::aisc();
    let at = |designation: &str, point: CardinalPoint| {
        let section = library.section(designation, "s").unwrap();
        let shape = section.shape.as_ref().unwrap();
        (
            point.centroid_offset(&section).unwrap(),
            shape.properties.clone(),
        )
    };
    // A doubly symmetric W: the corners are half the depth and width away.
    let ([y, z], p) = at("W14x90", CardinalPoint::BottomLeft);
    assert!((y - p[&SectionProperty::Depth] / 2.0).abs() < 1e-15);
    assert!((z - p[&SectionProperty::FlangeWidth] / 2.0).abs() < 1e-15);
    let ([y, z], _) = at("W14x90", CardinalPoint::MiddleCenter);
    assert_eq!([y, z], [0.0, 0.0]);
    // A channel's centroid sits x-bar from the back of its web, at the left.
    let ([_, z], p) = at("C10x15.3", CardinalPoint::MiddleLeft);
    assert!((z - p[&SectionProperty::CentroidX]).abs() < 1e-15);
    // A tee's centroid sits y-bar below the top of its flange.
    let ([y, _], p) = at("WT7x45", CardinalPoint::TopCenter);
    assert!((y + p[&SectionProperty::CentroidY]).abs() < 1e-15);
    // A round tube is its diameter square.
    let ([y, z], p) = at("HSS10.000x0.500", CardinalPoint::TopRight);
    let od = p[&SectionProperty::OutsideDiameter];
    assert!((y + od / 2.0).abs() < 1e-15 && (z + od / 2.0).abs() < 1e-15);
    // A section with no shape has nothing to measure but its centroid.
    let plain = Section {
        shape: None,
        ..library.section("W14x90", "s").unwrap()
    };
    assert!(CardinalPoint::TopCenter.centroid_offset(&plain).is_none());
    assert_eq!(
        CardinalPoint::Centroid.centroid_offset(&plain),
        Some([0.0; 2])
    );
}

#[test]
fn frame_offsets_are_validated_compiled_and_undone() {
    let mut editor = Editor::new(Model::default());
    portal(&mut editor);
    let beam = editor.model.find::<Frame>("B1").unwrap();
    let original = editor.model.frames[&beam].clone();
    for bad in [
        FrameOffsets {
            rigid_zone: 1.5,
            ..Default::default()
        },
        FrameOffsets {
            end: [Length::from_si(-0.1), Length::ZERO],
            ..Default::default()
        },
        FrameOffsets {
            joint: [[Length::from_si(f64::NAN), Length::ZERO, Length::ZERO]; 2],
            ..Default::default()
        },
    ] {
        let mut probe = editor.model.clone();
        let mut edited = original.clone();
        edited.offsets = bad;
        let err = Command::UpdateFrame {
            id: beam,
            frame: edited,
        }
        .apply(&mut probe)
        .unwrap_err();
        assert!(err.to_string().contains("offsets must be finite"), "{err}");
    }
    // End offsets that swallow the member are a problem on that frame.
    let mut long = original.clone();
    long.offsets.end = [Length::from_si(1e3), Length::ZERO];
    editor
        .apply(Command::UpdateFrame {
            id: beam,
            frame: long,
        })
        .unwrap();
    let problems = compile(&editor.model).unwrap_err();
    assert!(
        problems.iter().any(|p| p.entity == Some(beam)),
        "{problems:?}"
    );
    assert!(editor.undo().unwrap());
    // A cardinal point on a section with no shape is a problem too.
    let mut hung = original.clone();
    hung.cardinal_point = CardinalPoint::TopCenter;
    let mut plain = editor.model.sections[&original.section].clone();
    plain.shape = None;
    editor
        .apply(Command::UpdateSection {
            id: original.section,
            section: plain,
        })
        .unwrap();
    editor
        .apply(Command::UpdateFrame {
            id: beam,
            frame: hung.clone(),
        })
        .unwrap();
    let problems = compile(&editor.model).unwrap_err();
    assert!(
        problems
            .iter()
            .any(|p| p.entity == Some(beam) && p.message.contains("cardinal point top center")),
        "{problems:?}"
    );
    // Its length says why too, rather than passing for zero.
    let err = frame_length(&editor.model, beam).unwrap_err();
    assert!(err.contains("cardinal point top center"), "{err}");
    assert!(editor.undo().unwrap());
    assert!(editor.undo().unwrap());
    // A rigid zone stiffens the beam and compiles straight through.
    let mut stiff = original.clone();
    stiff.offsets.end = [Length::from_si(0.5), Length::from_si(0.5)];
    stiff.offsets.rigid_zone = 1.0;
    editor
        .apply(Command::UpdateFrame {
            id: beam,
            frame: stiff.clone(),
        })
        .unwrap();
    let compiled = compile(&editor.model).unwrap();
    assert_eq!(
        compiled.solver.frames[compiled.mapping.frame_index[&beam]].offsets,
        stiff.offsets
    );
    assert!(editor.undo().unwrap());
    assert_eq!(editor.model.frames[&beam], original);
}
