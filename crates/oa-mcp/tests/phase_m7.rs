//! Phase M7: the ASCE 7 generator, modal and response-spectrum tools, and
//! the guards that let an agent share a model with a person.
use oa_mcp::{Quantity, Session, SpectrumRequest};
use oa_model::{Command, EntityKind, LoadCase, LoadType, Material, asce7};
use serde_json::{Value, json};
use std::f64::consts::TAU;

fn cmd(v: Value) -> Command {
    serde_json::from_value(v).unwrap()
}
fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1e-12)
}

// A 12 ft massless cantilever column carrying 1 kip·s²/ft at its tip in X
// only: one dynamic degree of freedom, with stiffness 3EI/L³.
const E_KSI: f64 = 29_000.0;
const I_IN4: f64 = 1_000.0;
const L_IN: f64 = 144.0;
const MASS_KIP_S2_PER_FT: f64 = 1.0;

/// Stiffness in kip/in and mass in kip·s²/in.
fn sdof() -> (f64, f64) {
    (
        3.0 * E_KSI * I_IN4 / L_IN.powi(3),
        MASS_KIP_S2_PER_FT / 12.0,
    )
}

/// The cantilever, with the ids of its tip node, base node, and column.
fn cantilever() -> (Session, [oa_model::EntityId; 3]) {
    let mut s = Session::default();
    let base = s.find(EntityKind::Level, "Base").unwrap();
    let [top, n0, n1, mat, sec, col] = <[_; 6]>::try_from(s.next_ids(6)).unwrap();
    s.apply(vec![
        cmd(json!({"command": "add_level", "id": top, "level": {"name": "Top", "elevation": 12.0}})),
        cmd(json!({"command": "add_node", "id": n0, "node": {"name": "B", "level": base, "position": [0, 0, 0], "restrained": vec![true; 6]}})),
        cmd(json!({"command": "add_node", "id": n1, "node": {"name": "T", "level": top, "position": [0, 0, 12], "restrained": vec![false; 6], "mass": [MASS_KIP_S2_PER_FT, 0, 0]}})),
        cmd(json!({"command": "add_material", "id": mat, "material": {"name": "massless", "young": E_KSI, "poisson": 0.3, "density": 0}})),
        cmd(json!({"command": "add_section", "id": sec, "section": {"name": "col", "area": 20.0, "iy": I_IN4, "iz": I_IN4, "torsion": 10.0}})),
        cmd(json!({"command": "add_frame", "id": col, "frame": {"name": "C", "nodes": [n0, n1], "material": mat, "section": sec}})),
    ])
    .unwrap();
    (s, [n1, n0, col])
}

#[test]
fn modal_period_of_a_cantilever_with_a_tip_mass() {
    let (mut s, _) = cantilever();
    let (k, m) = sdof();
    let result = s.modal(1).unwrap();
    let mode = &result["modes"][0];
    let period = mode["period"].as_f64().unwrap();
    assert!(close(period, TAU * (m / k).sqrt(), 1e-6), "{result:#}");
    assert!(close(mode["mass_ratio"]["x"].as_f64().unwrap(), 1.0, 1e-9));
    assert!(close(
        result["total_free_mass"][0].as_f64().unwrap(),
        MASS_KIP_S2_PER_FT,
        1e-9
    ));
    assert_eq!(result["sturm_check"]["passed"], true);
}

fn flat_spectrum(sa: f64) -> SpectrumRequest {
    serde_json::from_value(json!({
        "spectrum": [[0.0, sa], [10.0, sa]],
        "direction": "x",
        "modes": 1,
    }))
    .unwrap()
}

#[test]
fn response_spectrum_of_one_degree_of_freedom() {
    let (mut s, [tip, base, column]) = cantilever();
    let (k, m) = sdof();
    let sa = 0.5 * 386.088_6; // in/s² at the model's default gravity
    let result = s.response_spectrum(&flat_spectrum(0.5)).unwrap();

    // Peak displacement Sa/ω² and base shear m·Sa.
    let peak = sa * m / k;
    assert!(close(
        result["captured_mass_ratio"].as_f64().unwrap(),
        1.0,
        1e-9
    ));
    assert!(
        close(
            result["base_reaction"]["fx"].as_f64().unwrap().abs(),
            m * sa,
            1e-6
        ),
        "{result:#}"
    );
    let largest = &result["largest_displacements"][0]["rows"][0];
    assert_eq!(largest["name"], "T");
    assert!(close(largest["peak"].as_f64().unwrap(), peak, 1e-6));

    let at = |s: &Session, quantity, component, id| {
        s.spectrum_peaks(quantity, component, Some(id), None, 1)
            .unwrap()["rows"][0]["peak"]
            .as_f64()
            .unwrap()
    };
    assert!(close(at(&s, Quantity::Displacement, "ux", tip), peak, 1e-6));
    assert!(close(at(&s, Quantity::Reaction, "ux", base), m * sa, 1e-6));
    // The column's base moment is the shear times its height, in kip·ft.
    assert!(close(
        at(&s, Quantity::FrameForce, "my_i", column).max(at(
            &s,
            Quantity::FrameForce,
            "mz_i",
            column
        )),
        m * sa * 12.0,
        1e-6
    ));
    // A node is not a frame.
    assert!(
        s.spectrum_peaks(Quantity::FrameForce, "n_i", Some(tip), None, 1)
            .is_err()
    );

    // Any edit discards the peaks.
    s.apply(vec![cmd(
        json!({"command": "set_gravity", "gravity": 32.2}),
    )])
    .unwrap();
    let err = s
        .spectrum_peaks(Quantity::Displacement, "ux", None, None, 5)
        .unwrap_err();
    assert!(err.to_string().contains("run response_spectrum"));
}

#[test]
fn an_agent_cannot_undo_a_persons_edit() {
    let mut s = Session::default();
    let id = s.next_ids(1)[0];
    s.apply(vec![cmd(json!({"command": "add_material", "id": id, "material": {"name": "agent's", "young": 29000, "poisson": 0.3, "density": 490}}))]).unwrap();
    // The person adds a material of their own, in SI, through the GUI path.
    let theirs = s.next_ids(1)[0];
    let material: Material =
        serde_json::from_value(json!({"name": "person's", "young": 2e11, "poisson": 0.3})).unwrap();
    s.user_apply(Command::AddMaterial {
        id: theirs,
        material,
    })
    .unwrap();
    let err = s.undo().unwrap_err().to_string();
    assert!(err.contains("not yours"), "{err}");
    assert!(s.redo().is_err());
    assert_eq!(s.model().materials.len(), 2, "nothing was undone");
    // The person can undo their own edit, and the agent, once it has made
    // a change of its own again, can undo that.
    assert!(s.user_undo().unwrap());
    let more = s.next_ids(1)[0];
    s.apply(vec![cmd(
        json!({"command": "add_group", "id": more, "group": {"name": "g"}}),
    )])
    .unwrap();
    assert!(s.undo().unwrap());
    assert!(s.model().groups.is_empty());
}

#[test]
fn an_agents_undo_stops_at_a_persons_edit() {
    let mut s = Session::default();
    let named = |name: &str| cmd(json!({"command": "set_metadata", "metadata": {"name": name}}));
    s.user_apply(named("person's")).unwrap();
    s.apply(vec![named("agent's")]).unwrap();
    // The agent's own undo leaves the person's edit on top of the stack,
    // and a second undo must not take it.
    assert!(s.undo().unwrap());
    let err = s.undo().unwrap_err().to_string();
    assert!(err.contains("not yours"), "{err}");
    assert_eq!(s.model().metadata.name, "person's");
    // The agent's undone change is still its own to redo.
    assert!(s.redo().unwrap());
    assert_eq!(s.model().metadata.name, "agent's");
}

#[test]
fn unsaved_work_is_protected_only_where_asked() {
    let dir = std::env::temp_dir().join(format!("oa-mcp-m7-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("m.oa.json");

    // Headless: an agent may discard its own work.
    let mut s = Session::default();
    s.next_ids(1);
    s.apply(vec![cmd(
        json!({"command": "set_gravity", "gravity": 32.2}),
    )])
    .unwrap();
    assert!(s.is_dirty());
    s.new_model("fresh").unwrap();
    assert!(!s.is_dirty());

    // In the GUI, a person's unsaved change blocks replacing the model.
    let mut s = Session::default();
    s.protect_unsaved = true;
    s.user_apply(Command::SetGravity {
        gravity: oa_core::units::Acceleration::from_si(9.8),
    })
    .unwrap();
    let generation = s.generation();
    assert!(
        s.new_model("fresh")
            .unwrap_err()
            .to_string()
            .contains("unsaved")
    );
    assert!(s.load(path.clone()).is_err());
    assert_eq!(s.generation(), generation);
    s.save(Some(path.clone())).unwrap();
    assert!(!s.is_dirty());
    s.new_model("fresh").unwrap();
    assert_eq!(s.generation(), generation + 1);
    assert!(
        s.protect_unsaved,
        "the replacement keeps the host's settings"
    );
    s.load(path.clone()).unwrap();
    assert_eq!(s.path(), Some(path.as_path()));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn results_are_copied_to_memory_only_on_request() {
    let (mut s, [tip, ..]) = cantilever();
    let [case, combo] = <[_; 2]>::try_from(s.next_ids(2)).unwrap();
    s.apply(vec![
        cmd(json!({"command": "add_load_case", "id": case, "load_case": {"name": "W", "nodal": [{"node": tip, "force": [1, 0, 0]}]}})),
        cmd(json!({"command": "add_combination", "id": combo, "combination": {"name": "W", "terms": [[case, 1.0]]}})),
    ])
    .unwrap();
    s.analyze(Default::default(), None).unwrap();
    assert!(s.take_fresh_results().is_none());

    s.keep_in_memory = true;
    s.analyze(Default::default(), None).unwrap();
    let (compiled, results) = s.take_fresh_results().unwrap();
    assert_eq!(results.combinations.len(), 1);
    assert_eq!(compiled.solver.combinations.len(), 1);
    assert!(s.take_fresh_results().is_none(), "handed out once");
    // The store still answers queries.
    assert!(s.envelope(tip, Quantity::Displacement, "ux").is_ok());
}

#[test]
fn generated_combinations_are_one_undo_step() {
    let mut s = Session::default();
    let [d, l] = <[_; 2]>::try_from(s.next_ids(2)).unwrap();
    s.user_apply(Command::AddLoadCase {
        id: d,
        load_case: LoadCase::new("D").with_type(LoadType::Dead),
    })
    .unwrap();
    s.user_apply(Command::AddLoadCase {
        id: l,
        load_case: LoadCase::new("L").with_type(LoadType::Live),
    })
    .unwrap();
    let added = s
        .generate_combinations(asce7::Edition::Asce7_22, asce7::Method::Strength)
        .unwrap();
    assert_eq!(added["added"], json!(["1.4D", "1.2D + 1.6L"]));
    assert_eq!(s.model().combinations.len(), 2);
    let again = s
        .generate_combinations(asce7::Edition::Asce7_22, asce7::Method::Strength)
        .unwrap();
    assert_eq!(again["added"], json!([]));
    assert!(again["note"].as_str().unwrap().contains("already has"));
    assert!(s.undo().unwrap());
    assert!(s.model().combinations.is_empty());

    // Cases without a type are named in the note.
    let mut s = Session::default();
    let id = s.next_ids(1)[0];
    s.apply(vec![cmd(
        json!({"command": "add_load_case", "id": id, "load_case": {"name": "misc"}}),
    )])
    .unwrap();
    let none = s
        .generate_combinations(asce7::Edition::Asce7_16, asce7::Method::AllowableStress)
        .unwrap();
    assert!(none["note"].as_str().unwrap().contains("load_type"));
}
