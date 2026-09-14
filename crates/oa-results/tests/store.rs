//! Acceptance: the SQLite store round-trips every result and its envelopes
//! match the in-memory `Envelope` on the benchmark moment frame.
use oa_core::{units::*, *};
use oa_results::{ResultStore, model_hash};

/// Same regular moment frame as scripts/benchmark_pynite.py: Y up, 6 m bays, 4 m storeys.
fn frame_model(bx: usize, by: usize, stories: usize, combos: usize) -> Model {
    let (nx, ny, nz) = (bx + 1, by + 1, stories + 1);
    let index = |i: usize, j: usize, k: usize| k * ny * nx + j * nx + i;
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_gpa(200.0),
        poisson: 0.3,
        density: MassDensity::from_si(7850.0),
    });
    m.add_section(Section {
        area: Area::from_si(0.01),
        iy: SecondMoment::from_si(2e-5),
        iz: SecondMoment::from_si(4e-5),
        torsion: SecondMoment::from_si(1e-5),
    });
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let p = [
                    Length::from_si(6.0 * i as f64),
                    Length::from_si(4.0 * k as f64),
                    Length::from_si(6.0 * j as f64),
                ];
                m.add_node(if k == 0 { Node::fixed(p) } else { Node::new(p) });
            }
        }
    }
    let mut beams = vec![];
    for k in 0..stories {
        for j in 0..ny {
            for i in 0..nx {
                m.add_frame(Frame::new(
                    [NodeId(index(i, j, k)), NodeId(index(i, j, k + 1))],
                    MaterialId(0),
                    SectionId(0),
                ));
            }
        }
    }
    for k in 1..nz {
        for j in 0..ny {
            for i in 0..bx {
                beams.push(m.add_frame(Frame::new(
                    [NodeId(index(i, j, k)), NodeId(index(i + 1, j, k))],
                    MaterialId(0),
                    SectionId(0),
                )));
            }
        }
        for j in 0..by {
            for i in 0..nx {
                beams.push(m.add_frame(Frame::new(
                    [NodeId(index(i, j, k)), NodeId(index(i, j + 1, k))],
                    MaterialId(0),
                    SectionId(0),
                )));
            }
        }
    }
    let dead = m.add_load_case(LoadCase {
        name: "dead".into(),
        member: beams
            .iter()
            .map(|&b| MemberLoad::Distributed {
                member: b,
                start: Length::ZERO,
                end: Length::from_si(6.0),
                start_load: [LineLoad::ZERO, LineLoad::from_si(-10_000.0), LineLoad::ZERO],
                end_load: [LineLoad::ZERO, LineLoad::from_si(-10_000.0), LineLoad::ZERO],
                axes: Axes::Global,
            })
            .collect(),
        ..Default::default()
    });
    let wind = m.add_load_case(LoadCase {
        name: "wind".into(),
        nodal: (1..nz)
            .flat_map(|k| (0..ny).flat_map(move |j| (0..nx).map(move |i| (i, j, k))))
            .map(|(i, j, k)| {
                NodalLoad::force(
                    NodeId(index(i, j, k)),
                    [Force::from_si(5_000.0), Force::ZERO, Force::ZERO],
                )
            })
            .collect(),
        ..Default::default()
    });
    for c in 0..combos {
        m.combinations.push(LoadCombination {
            name: format!("c{c}"),
            terms: vec![
                (dead, 1.0 + 0.05 * c as f64),
                (wind, (-1.0f64).powi(c as i32) * (0.5 + 0.1 * c as f64)),
            ],
        });
    }
    m
}

fn memory_envelope<'a>(
    results: &'a InMemoryResults,
    pick: impl Fn(&'a CombinationResult) -> f64,
) -> Envelope {
    Envelope::from_values(
        results
            .combinations
            .iter()
            .map(|c| (c.combination.as_str(), pick(c))),
    )
    .unwrap()
}
fn same(a: &Envelope, b: &Envelope) {
    assert_eq!(a.minimum.combination, b.minimum.combination);
    assert_eq!(a.maximum.combination, b.maximum.combination);
    assert_eq!(a.minimum.value, b.minimum.value);
    assert_eq!(a.maximum.value, b.maximum.value);
}

#[test]
fn store_round_trips_results_and_matches_memory_envelopes() {
    let model = frame_model(4, 4, 4, 8);
    let options = StaticOptions::default();
    let memory = analyze_static(&model, &options).unwrap();
    let mut store = ResultStore::create_in_memory(&model, &options).unwrap();
    analyze_static_into(&model, &options, &mut store).unwrap();

    assert_eq!(store.combinations().len(), 8);
    assert!(store.matches(&model).unwrap());
    let mut other = model.clone();
    other.gravity = Acceleration::from_si(9.81);
    assert!(!store.matches(&other).unwrap());
    assert_eq!(store.info().model_hash, model_hash(&model).unwrap());

    for c in &memory.combinations {
        let stored = store.combination(&c.combination).unwrap();
        assert_eq!(stored.displacements, c.displacements);
        assert_eq!(stored.reactions, c.reactions);
        let (a, b) = (stored.frames.unwrap(), c.frames.clone().unwrap());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.active, y.active);
            assert_eq!(x.local_end_forces, y.local_end_forces);
            assert_eq!(x.local_displacements, y.local_displacements);
        }
    }
    let top = model.nodes.len() - 1;
    for component in 0..6 {
        same(
            &store.envelope_displacement(top, component).unwrap(),
            &memory_envelope(&memory, |c| {
                c.displacements.as_ref().unwrap()[top][component]
            }),
        );
        same(
            &store.envelope_reaction(0, component).unwrap(),
            &memory_envelope(&memory, |c| c.reactions.as_ref().unwrap()[0][component]),
        );
    }
    for component in 0..12 {
        same(
            &store.envelope_frame_force(7, component).unwrap(),
            &memory_envelope(&memory, |c| {
                c.frames.as_ref().unwrap()[7].local_end_forces[component]
            }),
        );
    }
    let below = top - 25;
    same(
        &store.envelope_drift(top, below, 0).unwrap(),
        &memory_envelope(&memory, |c| {
            let d = c.displacements.as_ref().unwrap();
            d[top][0] - d[below][0]
        }),
    );
    let frame = store.frame_result("c3", 7).unwrap().unwrap();
    assert_eq!(
        frame.local_end_forces,
        memory.combinations[3].frames.as_ref().unwrap()[7].local_end_forces
    );
    assert!(store.frame_result("c3", 99_999).unwrap().is_none());
    let grouped = store.envelope_displacements(&[top, below], 1).unwrap();
    assert_eq!(grouped.len(), 2);
}

#[test]
fn sql_is_read_only_and_bounded() {
    let model = frame_model(2, 2, 2, 3);
    let options = StaticOptions::default();
    let mut store = ResultStore::create_in_memory(&model, &options).unwrap();
    analyze_static_into(&model, &options, &mut store).unwrap();
    let t = store
        .sql(
            "select c.name, max(abs(d.ux)) as peak from displacements d join combinations c on c.id = d.combination group by c.name order by peak desc",
            2,
        )
        .unwrap();
    assert_eq!(t.columns, vec!["name", "peak"]);
    assert_eq!(t.rows.len(), 2);
    assert!(t.truncated);
    assert!(store.sql("delete from combinations", 10).is_err());
    assert!(store.sql("not sql", 10).is_err());
    let err = analyze_static_into(&model, &options, &mut store).unwrap_err();
    assert!(
        matches!(err, Error::Consumer(_)),
        "duplicate combos rejected"
    );
}

#[test]
fn open_reads_back_a_file_store() {
    let dir = std::env::temp_dir().join(format!("oa-results-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.sqlite");
    let model = frame_model(2, 2, 2, 2);
    let options = StaticOptions {
        outputs: OutputSelection {
            reactions: false,
            ..Default::default()
        },
        ..Default::default()
    };
    {
        let mut store = ResultStore::create(&path, &model, &options).unwrap();
        analyze_static_into(&model, &options, &mut store).unwrap();
    }
    let store = ResultStore::open(&path).unwrap();
    assert_eq!(store.combinations(), vec!["c0", "c1"]);
    assert!(store.matches(&model).unwrap());
    let c = store.combination("c1").unwrap();
    assert!(c.displacements.is_some());
    assert!(c.reactions.is_none(), "deselected output stays absent");
    let options_back: StaticOptions = serde_json::from_str(&store.info().options).unwrap();
    assert!(!options_back.outputs.reactions);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Timing comparison from the Phase 8 acceptance criteria. Run in release:
/// cargo test -p oa-results --release -- --ignored --nocapture
#[test]
#[ignore]
fn timing_sqlite_write_versus_json() {
    // OA_BENCH_SIZE="bx,by,stories,combos" overrides the default frame.
    let size: Vec<usize> = std::env::var("OA_BENCH_SIZE")
        .ok()
        .map(|s| s.split(',').map(|v| v.parse().unwrap()).collect())
        .unwrap_or_else(|| vec![10, 10, 16, 32]);
    let model = frame_model(size[0], size[1], size[2], size[3]);
    println!(
        "frame {}x{}x{} with {} combos: {} nodes, {} frames",
        size[0],
        size[1],
        size[2],
        size[3],
        model.nodes.len(),
        model.frames.len()
    );
    let options = StaticOptions::default();
    let t = std::time::Instant::now();
    let memory = analyze_static(&model, &options).unwrap();
    let solve = t.elapsed();
    let t = std::time::Instant::now();
    let json = serde_json::to_string(&memory.combinations).unwrap();
    let json_time = t.elapsed();
    let dir = std::env::temp_dir().join(format!("oa-results-timing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let t = std::time::Instant::now();
    let mut store = ResultStore::create(dir.join("run.sqlite"), &model, &options).unwrap();
    for c in memory.combinations {
        store.consume(c).unwrap();
    }
    let sqlite_time = t.elapsed();
    let t = std::time::Instant::now();
    let env = store.envelope_frame_force(100, 5).unwrap();
    let query_time = t.elapsed();
    let size = std::fs::metadata(dir.join("run.sqlite")).unwrap().len();
    println!(
        "solve {solve:?}, json {json_time:?} ({} MB), sqlite write {sqlite_time:?} ({} MB), envelope query {query_time:?} ({:?})",
        json.len() / 1_000_000,
        size / 1_000_000,
        env.maximum.combination
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
