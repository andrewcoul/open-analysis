//! Diagnostic probes for the audit. These assert the observed defects, not
//! desired behavior. Copy to crates/oa-model/tests/audit_probe.rs to run.
use oa_core::{units::*, *};
use oa_model::{Command, Editor, EntityId};
use oa_results::ResultStore;

fn beam() -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_si(200e9),
        poisson: 0.3,
        density: MassDensity::ZERO,
    });
    m.add_section(Section {
        area: Area::from_si(0.01),
        iy: SecondMoment::from_si(2e-5),
        iz: SecondMoment::from_si(4e-5),
        torsion: SecondMoment::from_si(1e-5),
    });
    m.add_node(Node::fixed([Length::ZERO; 3]));
    m.add_node(Node::new([Length::from_si(3.0), Length::ZERO, Length::ZERO]));
    m.add_frame(Frame::new([NodeId(0), NodeId(1)], MaterialId(0), SectionId(0)));
    m.add_load_case(LoadCase {
        name: "good".into(),
        nodal: vec![NodalLoad::force(NodeId(1), [Force::ZERO, Force::from_si(-1000.0), Force::ZERO])],
        ..Default::default()
    });
    m
}

fn path(name: &str) -> std::path::PathBuf {
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/audit-probes")
        .join(format!("{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&directory).unwrap();
    directory.join(name)
}

#[test]
fn nonlinear_spring_fails_despite_linear_equilibrium() {
    let mut m = beam();
    m.nodes[1].spring_translation[1] = Stiffness::from_si(1e6);
    let linear = analyze_static(&m, &StaticOptions::default()).unwrap();
    let u = linear.combinations[0].displacements.as_ref().unwrap()[1][1];
    assert!((u - (-1000.0 / (1e6 + 3.0 * 200e9 * 4e-5 / 27.0))).abs() < 1e-12);
    for method in [StaticMethod::Nonlinear, StaticMethod::PDelta] {
        let error = analyze_static(&m, &StaticOptions { method, max_iterations: 3, ..Default::default() }).unwrap_err();
        println!("{method:?}: {error}; exact linear displacement={u}");
        assert!(matches!(error, Error::NonConvergence { .. }));
    }
}

#[test]
fn restrained_diaphragm_master_loses_support_reaction() {
    let mut m = beam();
    m.add_node(Node::fixed([Length::from_si(3.0), Length::from_si(1.0), Length::ZERO]));
    m.diaphragms.push(Diaphragm { master: NodeId(2), nodes: vec![NodeId(1)], normal: Axis::Z });
    m.load_cases[0].nodal[0].force = [Force::from_si(1000.0), Force::ZERO, Force::ZERO];
    let result = analyze_static(&m, &StaticOptions::default()).unwrap();
    let c = &result.combinations[0];
    let rx: f64 = c.reactions.as_ref().unwrap().iter().map(|r| r[0]).sum();
    println!("diaphragm: applied Fx=1000 N, sum support Rx={rx} N, residual={}", c.relative_residual);
    assert!(rx.abs() < 1e-10, "observed missing -1000 N reaction");
    assert!(c.relative_residual < 1e-10);
}

#[test]
fn failed_run_reopens_and_attaches_with_partial_envelope() {
    let mut m = beam();
    m.frames[0].releases[3] = true;
    m.frames[0].releases[9] = true;
    m.add_load_case(LoadCase {
        name: "bad".into(),
        nodal: vec![NodalLoad { node: NodeId(1), force: [Force::ZERO; 3], moment: [Moment::from_si(100.0), Moment::ZERO, Moment::ZERO] }],
        ..Default::default()
    });
    let compiled = oa_model::compile(&oa_model::Model::from_solver(&m)).unwrap();
    let db = path("partial.sqlite");
    let options = StaticOptions::default();
    let mut store = ResultStore::create(&db, &m, &options).unwrap();
    let error = analyze_static_into(&m, &options, &mut store).unwrap_err();
    drop(store);
    let store = ResultStore::open(&db).unwrap();
    assert!(store.matches(&m).unwrap());
    oa_model::store::attach(&compiled, &store).unwrap();
    assert_eq!(store.combinations(), vec!["good"]);
    let envelope = store.envelope_displacement(1, 1).unwrap();
    println!("partial store: error={error}; reopened+attached with {:?}, envelope={:?}", store.combinations(), envelope);
}

#[test]
fn rejected_command_is_replayed_and_blocks_later_edits() {
    let db = path("journal.sqlite");
    let mut editor = Editor::new(oa_model::Model::default())
        .with_journal(oa_model::store::Journal::open(&db).unwrap());
    editor.apply(Command::AddNode { id: EntityId(1), node: oa_model::Node::new("A", [Length::ZERO; 3]) }).unwrap();
    assert!(editor.apply(Command::AddNode { id: EntityId(2), node: oa_model::Node::new("A", [Length::ZERO; 3]) }).is_err());
    editor.apply(Command::AddNode { id: EntityId(3), node: oa_model::Node::new("B", [Length::ZERO; 3]) }).unwrap();
    drop(editor);
    let journal = oa_model::store::Journal::open(&db).unwrap();
    let commands = journal.replay().unwrap();
    assert_eq!(commands.len(), 3);
    let mut recovered = oa_model::Model::default();
    let error = commands.into_iter().try_for_each(|c| c.apply(&mut recovered).map(|_| ())).unwrap_err();
    assert_eq!(recovered.nodes.len(), 1);
    println!("journal: recovery stops at {error}; later accepted node B not recovered");
}

#[test]
fn loaded_allocator_can_overwrite_an_existing_node() {
    let mut m = oa_model::Model::from_solver(&beam());
    m.next_id = 1;
    let mut loaded = oa_model::from_json(&oa_model::to_json(&m)).unwrap();
    oa_model::compile(&loaded).unwrap();
    let original = loaded.nodes[&EntityId(1)].name.clone();
    let id = loaded.insert(oa_model::Node::new("replacement", [Length::ZERO; 3]));
    assert_eq!(id, EntityId(1));
    assert_eq!(loaded.nodes.len(), 2);
    println!("allocator: loaded next_id=1; insert silently replaced node {original:?}");
}

#[test]
fn read_only_sql_can_create_an_attached_database() {
    let m = beam();
    let store = ResultStore::create_in_memory(&m, &StaticOptions::default()).unwrap();
    let db = path("unexpected.sqlite");
    let statement = format!("ATTACH DATABASE '{}' AS extra", db.display().to_string().replace('\'', "''"));
    let response = store.sql(&statement, 10).unwrap();
    assert!(db.exists());
    println!("read-only SQL created {}: {:?}", db.display(), response);
}

#[test]
fn compile_accepts_impossible_shell_geometry() {
    let mut m = beam();
    m.frames.clear();
    m.load_cases.clear();
    m.shells.push(Shell {
        nodes: [NodeId(0); 4], material: MaterialId(0), thickness: Length::from_si(0.1),
        formulation: ShellFormulation::Dkmq, drilling_ratio: 1e-3,
    });
    m.add_load_case(LoadCase { name: "empty".into(), ..Default::default() });
    let compiled = oa_model::compile(&oa_model::Model::from_solver(&m)).unwrap();
    let error = analyze_static(&compiled.solver, &StaticOptions::default()).unwrap_err();
    println!("compile reports success for a shell with four identical nodes; solve: {error}");
}

#[test]
fn loaded_duplicate_identity_passes_compilation() {
    let mut m = oa_model::Model::from_solver(&beam());
    m.groups.insert(EntityId(1), oa_model::Group { name: "colliding group".into(), members: Default::default() });
    let loaded = oa_model::from_json(&oa_model::to_json(&m)).unwrap();
    oa_model::compile(&loaded).unwrap();
    assert_eq!(loaded.kind_of(EntityId(1)), Some(oa_model::EntityKind::Node));
    println!("duplicate identity: node and group #1 accepted, kind_of resolves only node");
}
