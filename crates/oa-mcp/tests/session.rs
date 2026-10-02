//! The Phase M6 acceptance scenario, driven through the session exactly as
//! the MCP tools would: build a two-storey frame from commands, run it, and
//! report the governing drift. No hand-written solver code. Z is up, with a
//! level per floor.
use oa_mcp::{Quantity, Session};
use oa_model::EntityKind;
use serde_json::json;

fn cmd(v: serde_json::Value) -> oa_model::Command {
    serde_json::from_value(v).unwrap()
}

#[test]
fn agent_scenario_two_storey_frame_to_governing_drift() {
    let mut s = Session::default();
    s.new_model("two storey").unwrap();
    let steel = s.add_material_from_library("A992", "steel").unwrap();
    let column = s.add_section_from_library("W14x90", "column").unwrap();
    let beam = s.add_section_from_library("W12x26", "beam").unwrap();

    // A new model has Base at 0; the two floors are levels at 12 and 24 ft.
    let base = s.find(EntityKind::Level, "Base").unwrap();
    let [l1, roof_level] = <[_; 2]>::try_from(s.next_ids(2)).unwrap();
    let mut commands = vec![
        cmd(json!({"command": "add_level", "id": l1, "level": {"name": "L1", "elevation": 12.0}})),
        cmd(json!({"command": "add_level", "id": roof_level, "level": {"name": "Roof", "elevation": 24.0}})),
    ];
    let levels = [base, base, l1, l1, roof_level, roof_level];

    // Six nodes: two fixed at grade, two per floor. A 20 ft bay, 12 ft storeys.
    let ids = s.next_ids(6);
    for (i, (x, z)) in [
        (0.0, 0.0),
        (20.0, 0.0),
        (0.0, 12.0),
        (20.0, 12.0),
        (0.0, 24.0),
        (20.0, 24.0),
    ]
    .into_iter()
    .enumerate()
    {
        commands.push(cmd(json!({
            "command": "add_node", "id": ids[i],
            "node": {"name": format!("N{i}"), "level": levels[i], "position": [x, 0.0, z], "restrained": vec![i < 2; 6]}
        })));
    }
    let frames = s.next_ids(6);
    for (k, (name, a, b, section)) in [
        ("C1", 0, 2, column),
        ("C2", 1, 3, column),
        ("C3", 2, 4, column),
        ("C4", 3, 5, column),
        ("B1", 2, 3, beam),
        ("B2", 4, 5, beam),
    ]
    .into_iter()
    .enumerate()
    {
        commands.push(cmd(json!({
            "command": "add_frame", "id": frames[k],
            "frame": {"name": name, "nodes": [ids[a], ids[b]], "material": steel, "section": section}
        })));
    }
    let [dead, wind, combo, roof] = <[_; 4]>::try_from(s.next_ids(4)).unwrap();
    commands.push(cmd(
        json!({"command": "add_load_case", "id": dead, "load_case": {
        "name": "dead", "self_weight": [0, 0, -1],
        "member": [{"type": "distributed", "member": frames[4], "start": 0, "end": 20,
                    "start_load": [0, 0, -1.5], "end_load": [0, 0, -1.5], "axes": "global"},
                   {"type": "distributed", "member": frames[5], "start": 0, "end": 20,
                    "start_load": [0, 0, -1.0], "end_load": [0, 0, -1.0], "axes": "global"}]}}),
    ));
    commands.push(cmd(json!({"command": "add_load_case", "id": wind, "load_case": {
        "name": "wind", "nodal": [{"node": ids[2], "force": [12, 0, 0]}, {"node": ids[4], "force": [8, 0, 0]}]}})));
    commands.push(cmd(
        json!({"command": "add_combination", "id": combo, "combination": {
        "name": "1.2D+1.0W", "terms": [[dead, 1.2], [wind, 1.0]]}}),
    ));
    commands.push(cmd(json!({"command": "add_group", "id": roof, "group": {"name": "roof", "members": [ids[4], ids[5], frames[5]]}})));
    let applied = s.apply(commands).unwrap();
    assert_eq!(applied["applied"], 18);
    assert_eq!(s.describe()["counts"]["frames"], 6);
    assert_eq!(s.describe()["counts"]["levels"], 3);

    let compiled = s.compile().unwrap();
    assert_eq!(compiled["ok"], true);
    let run = s.analyze(Default::default(), None).unwrap();
    assert_eq!(run["combinations"][0]["name"], "1.2D+1.0W");

    // The questions an agent would ask.
    let roof_drift = s.drift(ids[4], ids[2], "ux").unwrap();
    let storey_drift = s.drift(ids[2], ids[0], "ux").unwrap();
    assert!(roof_drift["maximum"]["value"].as_f64().unwrap() > 0.0);
    assert!(storey_drift["maximum"]["ratio"].as_f64().unwrap() > 0.0);
    assert_eq!(storey_drift["maximum"]["combination"], "1.2D+1.0W");
    // The ratio is over the Z separation: 12 ft of storey, dimensionless.
    assert!((storey_drift["height"].as_f64().unwrap() - 12.0).abs() < 1e-9);
    let drift_in = storey_drift["maximum"]["value"].as_f64().unwrap();
    let ratio = storey_drift["maximum"]["ratio"].as_f64().unwrap();
    assert!(
        (ratio - drift_in / (12.0 * 12.0)).abs() < 1e-9 * ratio.abs().max(1e-12),
        "ratio {ratio} vs {drift_in} in over 144 in"
    );
    let moments = s
        .group_envelope(roof, Quantity::FrameForce, "mz_j", 10)
        .unwrap();
    assert_eq!(
        moments["total"], 1,
        "only the roof beam is a frame in the group"
    );
    let base_reaction = s.envelope(ids[0], Quantity::Reaction, "uz").unwrap();
    assert!(
        base_reaction["maximum"]["value"].as_f64().unwrap() > 0.0,
        "base carries gravity"
    );
    let table = s
        .query("select count(*) as n from frame_results", 10)
        .unwrap();
    assert_eq!(table["rows"][0][0], 6);

    // Everything the agent sees is in US units: the model went in as typed,
    // and results come out in inches and kips whether asked through an
    // envelope or through SQL.
    let described = s.describe();
    assert_eq!(described["units"]["symbols"]["length"], "ft");
    assert!((described["gravity"].as_f64().unwrap() - 32.174).abs() < 1e-3);
    assert_eq!(described["levels"][1]["name"], "L1");
    assert!((described["levels"][2]["elevation"].as_f64().unwrap() - 24.0).abs() < 1e-9);
    let material = s.get(steel).unwrap();
    assert!((material["entity"]["young"].as_f64().unwrap() - 29_000.0).abs() < 1e-6);
    assert!((material["entity"]["density"].as_f64().unwrap() - 490.0).abs() < 1e-6);
    let top = s.get(ids[4]).unwrap();
    assert!((top["entity"]["position"][2].as_f64().unwrap() - 24.0).abs() < 1e-9);
    assert_eq!(top["entity"]["level"], roof_level.0);
    let listed = s.list(EntityKind::Node, Some("N5"), 1);
    assert!((listed["rows"][0]["position"][0].as_f64().unwrap() - 20.0).abs() < 1e-9);
    assert_eq!(listed["rows"][0]["level"], "Roof");
    assert_eq!(listed["rows"][0]["offset"], 0.0);
    let level_rows = s.list(EntityKind::Level, None, 10);
    assert_eq!(level_rows["total"], 3);
    assert_eq!(level_rows["rows"][0]["name"], "Base");
    assert!(level_rows["rows"][0]["height_below"].is_null());
    assert!((level_rows["rows"][2]["height_below"].as_f64().unwrap() - 12.0).abs() < 1e-9);
    assert_eq!(roof_drift["unit"], "in");
    let sway = s.envelope(ids[4], Quantity::Displacement, "ux").unwrap();
    assert_eq!(sway["unit"], "in");
    let index = s.entity_indices(&[ids[4]]).unwrap()[0]["node"]
        .as_u64()
        .unwrap();
    let via_sql = s
        .query(
            &format!("select ux from displacements where node = {index}"),
            1,
        )
        .unwrap();
    let sql_ux = via_sql["rows"][0][0].as_f64().unwrap();
    let envelope_ux = sway["maximum"]["value"].as_f64().unwrap();
    assert!(
        (sql_ux - envelope_ux).abs() < 1e-9 * envelope_ux.abs().max(1.0),
        "sql {sql_ux} vs envelope {envelope_ux}"
    );
    let reaction = s
        .query("select uz from reactions where node = 0", 1)
        .unwrap();
    let base_kip = reaction["rows"][0][0].as_f64().unwrap();
    let base_env = base_reaction["maximum"]["value"].as_f64().unwrap();
    assert!((base_kip - base_env).abs() < 1e-9 * base_env.abs().max(1.0));
    // Two beams at 1.5 and 1.0 kip/ft over 20 ft carry 50 kip, self weight
    // adds about 5 kip, and 1.2D puts about 66 kip on the two bases.
    let total = s.query("select sum(uz) from reactions", 1).unwrap()["rows"][0][0]
        .as_f64()
        .unwrap();
    assert!(
        total > 60.0 && total < 70.0,
        "total base reaction {total} kip"
    );
    // Naming the stored table directly would hand back newtons, so it is
    // refused rather than answered in the wrong units.
    let refused = s.query("select uz from main.reactions where node = 0", 1);
    let message = refused.unwrap_err().to_string();
    assert!(message.contains("US customary"), "{message}");
    let indices = s.entity_indices(&[ids[4], frames[5]]).unwrap();
    assert!(indices[0]["node"].is_number() && indices[1]["frame"].is_number());

    // Edits discard results, and undo restores the previous state.
    let extra = s.next_ids(1)[0];
    s.apply(vec![cmd(
        json!({"command": "add_node", "id": extra, "node": {"name": "N6", "level": base, "position": [3, 8, 0]}}),
    )])
    .unwrap();
    assert!(s.envelope(ids[4], Quantity::Displacement, "ux").is_err());
    assert!(s.undo().unwrap());
    assert_eq!(s.describe()["counts"]["nodes"], 6);
    assert_eq!(s.find(EntityKind::Frame, "B2"), Some(frames[5]));
    let listed = s.list(EntityKind::Frame, Some("C"), 2);
    assert_eq!(listed["total"], 4);
    assert_eq!(listed["truncated"], true);

    // Raising L1 in feet carries the roof and every node bound to either,
    // keeps the base still, and undoes to the exact document.
    let before = s.model().clone();
    s.apply(vec![cmd(json!({
        "command": "set_level_elevation", "id": l1, "elevation": 14.0, "scope": "this_and_above"
    }))])
    .unwrap();
    let z = |s: &Session, id: oa_model::EntityId| {
        s.get(id).unwrap()["entity"]["position"][2].as_f64().unwrap()
    };
    assert!((z(&s, ids[2]) - 14.0).abs() < 1e-9);
    assert!((z(&s, ids[4]) - 26.0).abs() < 1e-9);
    assert!((z(&s, ids[0])).abs() < 1e-9);
    let level_rows = s.list(EntityKind::Level, None, 10);
    assert!((level_rows["rows"][2]["elevation"].as_f64().unwrap() - 26.0).abs() < 1e-9);
    assert!((level_rows["rows"][2]["height_below"].as_f64().unwrap() - 12.0).abs() < 1e-9);
    assert!(s.undo().unwrap());
    assert_eq!(s.model(), &before);
    // A move that would cross the roof is refused and names the levels.
    let crossing = s.apply(vec![cmd(json!({
        "command": "set_level_elevation", "id": l1, "elevation": 30.0, "scope": "this_level"
    }))]);
    assert!(crossing.unwrap_err().to_string().contains("Roof"));
    // A level with nodes on it cannot be removed; one without them can.
    assert!(s.apply(vec![cmd(json!({"command": "remove_level", "id": l1}))]).is_err());
    let spare = s.next_ids(1)[0];
    s.apply(vec![
        cmd(json!({"command": "add_level", "id": spare, "level": {"name": "Mezzanine", "elevation": 6.0}})),
        cmd(json!({"command": "remove_level", "id": spare})),
    ])
    .unwrap();
    assert_eq!(s.describe()["counts"]["levels"], 3);

    // Problems are named, and a bad component is explained.
    let bad = s.apply(vec![cmd(json!({"command": "remove_node", "id": ids[0]}))]);
    assert!(bad.unwrap_err().to_string().contains("N0"));
    assert!(
        s.envelope(ids[4], Quantity::Displacement, "sideways")
            .is_err()
    );
}

#[test]
fn save_and_load_round_trip_through_the_session() {
    let dir = std::env::temp_dir().join(format!("oa-mcp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("model.json");
    let mut s = Session::default();
    s.new_model("saved").unwrap();
    let steel = s.add_material_from_library("A36", "steel").unwrap();
    assert!(steel.0 > 0);
    let saved = s.save(Some(path.clone())).unwrap();
    assert_eq!(saved, path);
    let mut t = Session::default();
    let described = t.load(path).unwrap();
    assert_eq!(described["name"], "saved");
    assert_eq!(described["counts"]["materials"], 1);
    assert_eq!(described["counts"]["levels"], 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn library_lists_aisc_shapes_and_copies_their_design_properties() {
    let mut s = Session::default();
    s.new_model("library").unwrap();
    let listed = s.library(Some("w14x9"), 50);
    assert_eq!(listed["sections"]["library"], "aisc-shapes");
    let found = listed["sections"]["designations"].as_array().unwrap();
    assert!(found.contains(&json!("W14X90")));
    assert_eq!(listed["sections"]["truncated"], false);
    let capped = s.library(None, 5);
    assert_eq!(
        capped["sections"]["designations"].as_array().unwrap().len(),
        5
    );
    assert_eq!(capped["sections"]["total"], 1523);
    assert_eq!(capped["sections"]["truncated"], true);
    let materials = capped["materials"]["designations"].as_array().unwrap();
    assert!(materials.contains(&json!("A992")));

    let entry = s.library_section("w14x90").unwrap();
    let shape = &entry["section"]["shape"];
    assert_eq!(shape["kind"], "W");
    assert!((shape["properties"]["Zx"].as_f64().unwrap() - 157.0).abs() < 1e-9);
    assert_eq!(entry["property_units"]["Zx"], "in³");
    assert_eq!(entry["property_units"]["Cw"], "in⁶");
    assert_eq!(entry["property_units"]["bf/2tf"], "");
    assert!(s.library_section("W99X999").is_err());

    let id = s.add_section_from_library("W14x90", "column").unwrap();
    let got = &s.get(id).unwrap()["entity"];
    assert_eq!(got["provenance"]["designation"], "W14X90");
    assert!((got["shape"]["properties"]["Sx"].as_f64().unwrap() - 143.0).abs() < 1e-9);
}

#[test]
fn materials_carry_strengths_in_ksi() {
    let mut s = Session::default();
    s.new_model("strengths").unwrap();
    let steel = s.add_material_from_library("A992", "steel").unwrap();
    let got = &s.get(steel).unwrap()["entity"];
    assert!((got["fy"].as_f64().unwrap() - 50.0).abs() < 1e-9);
    assert!((got["fu"].as_f64().unwrap() - 65.0).abs() < 1e-9);
    assert!(got.get("fc").is_none());

    let [concrete, bad] = <[_; 2]>::try_from(s.next_ids(2)).unwrap();
    s.apply(vec![cmd(
        json!({"command": "add_material", "id": concrete, "material":
        {"name": "concrete", "young": 3605, "poisson": 0.2, "density": 150, "fc": 4}}),
    )])
    .unwrap();
    let got = &s.get(concrete).unwrap()["entity"];
    assert!((got["fc"].as_f64().unwrap() - 4.0).abs() < 1e-9);
    let err = s
        .apply(vec![cmd(
            json!({"command": "add_material", "id": bad, "material":
            {"name": "bad", "young": 29000, "poisson": 0.3, "fy": 50, "fu": 40}}),
        )])
        .unwrap_err()
        .to_string();
    assert!(err.contains("Fu must be at least Fy"), "{err}");
}

#[test]
fn sections_carry_shear_areas_in_square_inches() {
    let mut s = Session::default();
    s.new_model("shear").unwrap();
    let entry = s.library_section("W14X90").unwrap();
    assert!((entry["section"]["shear_y"].as_f64().unwrap() - 6.16).abs() < 1e-9);
    let beam = s.add_section_from_library("W14X90", "beam").unwrap();
    let got = &s.get(beam).unwrap()["entity"];
    assert!((got["shear_z"].as_f64().unwrap() - 5.0 / 3.0 * 14.5 * 0.71).abs() < 1e-9);

    let [deep, bad] = <[_; 2]>::try_from(s.next_ids(2)).unwrap();
    s.apply(vec![cmd(
        json!({"command": "add_section", "id": deep, "section":
        {"name": "deep", "area": 20, "iy": 50, "iz": 900, "torsion": 2, "shear_y": 8}}),
    )])
    .unwrap();
    let got = &s.get(deep).unwrap()["entity"];
    assert!((got["shear_y"].as_f64().unwrap() - 8.0).abs() < 1e-9);
    assert!(got.get("shear_z").is_none());
    let err = s
        .apply(vec![cmd(
            json!({"command": "add_section", "id": bad, "section":
            {"name": "bad", "area": 20, "iy": 50, "iz": 900, "torsion": 2, "shear_z": 0}}),
        )])
        .unwrap_err()
        .to_string();
    assert!(err.contains("shear areas must be positive"), "{err}");
}

#[test]
fn frames_and_shells_carry_stiffness_modifiers() {
    let mut s = Session::default();
    s.new_model("cracked").unwrap();
    let concrete = s
        .add_material_from_library("Concrete 4 ksi", "concrete")
        .unwrap();
    let section = s.add_section_from_library("W14X90", "beam").unwrap();
    let base = s.find(EntityKind::Level, "Base").unwrap();
    let [a, b, beam, plain, bad] = <[_; 5]>::try_from(s.next_ids(5)).unwrap();
    s.apply(vec![
        cmd(json!({"command": "add_node", "id": a, "node": {"name": "A", "level": base, "position": [0, 0, 0]}})),
        cmd(json!({"command": "add_node", "id": b, "node": {"name": "B", "level": base, "position": [20, 0, 0]}})),
        cmd(json!({"command": "add_frame", "id": beam, "frame": {"name": "B1", "nodes": [a, b],
            "material": concrete, "section": section, "modifiers": {"iy": 0.35, "iz": 0.35}}})),
        cmd(json!({"command": "add_frame", "id": plain, "frame": {"name": "B2", "nodes": [a, b],
            "material": concrete, "section": section}})),
    ])
    .unwrap();
    let got = &s.get(beam).unwrap()["entity"]["modifiers"];
    assert_eq!(got["iz"], json!(0.35));
    assert_eq!(got["area"], json!(1.0));
    assert!(s.get(plain).unwrap()["entity"].get("modifiers").is_none());
    let err = s
        .apply(vec![cmd(json!({"command": "add_shell", "id": bad, "shell": {"name": "W1",
            "nodes": [a, b, b, a], "material": concrete, "thickness": 8, "modifiers": {"membrane_x": 0}}}))])
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("stiffness modifiers must be positive"),
        "{err}"
    );
}

#[test]
fn analyze_takes_a_service_stiffness_factor() {
    let mut s = Session::default();
    s.new_model("drift").unwrap();
    let steel = s.add_material_from_library("A992", "steel").unwrap();
    let base = s.find(EntityKind::Level, "Base").unwrap();
    let [section, a, b, column, wind] = <[_; 5]>::try_from(s.next_ids(5)).unwrap();
    s.apply(vec![
        cmd(json!({"command": "add_section", "id": section, "section":
            {"name": "col", "area": 20, "iy": 500, "iz": 500, "torsion": 5}})),
        cmd(
            json!({"command": "add_node", "id": a, "node": {"name": "A", "level": base,
            "position": [0, 0, 0], "restrained": [true, true, true, true, true, true]}}),
        ),
        cmd(
            json!({"command": "add_node", "id": b, "node": {"name": "B", "level": base,
            "position": [0, 0, 12]}}),
        ),
        cmd(
            json!({"command": "add_frame", "id": column, "frame": {"name": "C1", "nodes": [a, b],
            "material": steel, "section": section, "modifiers": {"iy": 0.35, "iz": 0.35}}}),
        ),
        cmd(
            json!({"command": "add_load_case", "id": wind, "load_case": {"name": "wind",
            "nodal": [{"node": b, "force": [1, 0, 0]}]}}),
        ),
    ])
    .unwrap();
    let drift = |s: &Session| {
        s.envelope(b, Quantity::Displacement, "ux").unwrap()["maximum"]["value"]
            .as_f64()
            .unwrap()
    };
    let strength = s.analyze(Default::default(), None).unwrap();
    assert_eq!(strength["cracked_stiffness_factor"], json!(1.0));
    let cracked = drift(&s);
    let options = oa_core::StaticOptions {
        cracked_stiffness_factor: 1.4,
        ..Default::default()
    };
    let service = s.analyze(options, None).unwrap();
    assert_eq!(service["cracked_stiffness_factor"], json!(1.4));
    assert!((cracked / drift(&s) - 1.4).abs() < 1e-9);
}

#[test]
fn mass_sources_turn_gravity_load_into_modal_mass() {
    let mut s = Session::default();
    s.new_model("mass").unwrap();
    let steel = s.add_material_from_library("A992", "steel").unwrap();
    let base = s.find(EntityKind::Level, "Base").unwrap();
    let [section, a, b, column, sdl, seismic, half] = <[_; 7]>::try_from(s.next_ids(7)).unwrap();
    s.apply(vec![
        cmd(json!({"command": "add_section", "id": section, "section":
            {"name": "col", "area": 20, "iy": 500, "iz": 500, "torsion": 5}})),
        cmd(
            json!({"command": "add_node", "id": a, "node": {"name": "A", "level": base,
            "position": [0, 0, 0], "restrained": [true, true, true, true, true, true]}}),
        ),
        cmd(
            json!({"command": "add_node", "id": b, "node": {"name": "B", "level": base,
            "position": [0, 0, 12]}}),
        ),
        cmd(
            json!({"command": "add_frame", "id": column, "frame": {"name": "C1", "nodes": [a, b],
            "material": steel, "section": section}}),
        ),
        cmd(
            json!({"command": "add_load_case", "id": sdl, "load_case": {"name": "SDL",
            "load_type": "dead", "nodal": [{"node": b, "force": [0, 0, -40]}]}}),
        ),
    ])
    .unwrap();
    let tip = |s: &mut Session, source: Option<&str>| {
        s.modal(1, source).unwrap()["total_free_mass"][0]
            .as_f64()
            .unwrap()
    };
    let own = tip(&mut s, None);
    assert_eq!(s.describe()["mass_sources"], json!([]));
    assert_eq!(s.describe()["default_mass_source"], json!(null));

    s.apply(vec![
        cmd(
            json!({"command": "add_mass_source", "id": seismic, "mass_source":
            {"name": "seismic", "cases": [[sdl, 1.0]], "vertical": false}}),
        ),
        cmd(
            json!({"command": "add_mass_source", "id": half, "mass_source":
            {"name": "half", "cases": [[sdl, 0.5]]}}),
        ),
        cmd(json!({"command": "set_default_mass_source", "id": seismic})),
    ])
    .unwrap();
    let described = s.describe();
    assert_eq!(described["default_mass_source"], json!("seismic"));
    let row = &described["mass_sources"][0];
    assert_eq!(
        row["cases"],
        json!([{"id": sdl, "name": "SDL", "multiplier": 1.0}])
    );
    assert_eq!(
        (&row["default"], &row["vertical"]),
        (&json!(true), &json!(false))
    );
    // 40 kip over g in ft/s², in kip·s²/ft, from the default source, and
    // half of it from the one asked for by name.
    let g = described["gravity"].as_f64().unwrap();
    assert!((tip(&mut s, None) - own - 40.0 / g).abs() < 1e-9);
    assert!((tip(&mut s, Some("half")) - own - 20.0 / g).abs() < 1e-9);
    let vertical = s.modal(1, None).unwrap()["total_free_mass"][2].clone();
    assert_eq!(vertical, json!(0.0));
    let err = s.modal(1, Some("nope")).unwrap_err().to_string();
    assert!(err.contains("no mass source named"), "{err}");
    for (command, refusal) in [
        (
            json!({"command": "remove_load_case", "id": sdl}),
            "mass source",
        ),
        (
            json!({"command": "remove_mass_source", "id": seismic}),
            "default",
        ),
    ] {
        let err = s.apply(vec![cmd(command)]).unwrap_err().to_string();
        assert!(err.contains(refusal), "{err}");
    }
}

#[test]
fn split_frames_connects_a_node_on_a_span() {
    let mut s = Session::default();
    s.new_model("split").unwrap();
    let steel = s.add_material_from_library("A992", "steel").unwrap();
    let section = s.add_section_from_library("W12x26", "beam").unwrap();
    let base = s.find(EntityKind::Level, "Base").unwrap();
    let [a, b, mid, beam, dead] = <[_; 5]>::try_from(s.next_ids(5)).unwrap();
    s.apply(vec![
        cmd(json!({"command": "add_node", "id": a, "node": {"name": "A", "level": base, "position": [0, 0, 0]}})),
        cmd(json!({"command": "add_node", "id": b, "node": {"name": "B", "level": base, "position": [20, 0, 0]}})),
        cmd(json!({"command": "add_node", "id": mid, "node": {"name": "M", "level": base, "position": [8, 0, 0]}})),
        cmd(json!({"command": "add_frame", "id": beam, "frame": {"name": "B1", "nodes": [a, b],
            "material": steel, "section": section}})),
        cmd(json!({"command": "add_load_case", "id": dead, "load_case": {"name": "D",
            "member": [{"type": "distributed", "member": beam, "start": 0, "end": 20,
                "start_load": [0, 0, -1], "end_load": [0, 0, -1], "axes": "global"}]}})),
    ])
    .unwrap();
    s.apply(vec![cmd(json!({"command": "split_frames"}))]).unwrap();
    assert_eq!(s.get(beam).unwrap()["entity"]["nodes"], json!([a, mid]));
    let second = s.find(EntityKind::Frame, "B1-2").unwrap();
    assert_eq!(s.get(second).unwrap()["entity"]["nodes"], json!([mid, b]));
    let loads = s.get(dead).unwrap()["entity"]["member"].clone();
    let loads = loads.as_array().unwrap();
    assert_eq!(loads.len(), 2);
    // Positions come back in ft, measured along each piece.
    assert!((loads[0]["end"].as_f64().unwrap() - 8.0).abs() < 1e-9);
    assert_eq!(loads[1]["member"], json!(second));
    assert!((loads[1]["end"].as_f64().unwrap() - 12.0).abs() < 1e-9);
    assert!((loads[1]["start_load"][2].as_f64().unwrap() + 1.0).abs() < 1e-9);
}

#[test]
fn grid_lines_go_in_and_come_back_in_feet() {
    let mut s = Session::default();
    s.new_model("grid").unwrap();
    let [a, one] = <[_; 2]>::try_from(s.next_ids(2)).unwrap();
    s.apply(vec![
        cmd(json!({"command": "add_grid_line", "id": a, "grid_line":
            {"name": "A", "start": [0, -5], "end": [0, 55]}})),
        cmd(json!({"command": "add_grid_line", "id": one, "grid_line":
            {"name": "1", "start": [-5, 0], "end": [95, 0]}})),
    ])
    .unwrap();
    let described = s.describe();
    assert_eq!(described["grid_lines"], json!(["A", "1"]));
    assert_eq!(described["counts"]["grid_lines"], json!(2));
    assert_eq!(s.find(EntityKind::GridLine, "1"), Some(one));
    let got = s.get(a).unwrap();
    assert_eq!(got["kind"], json!("grid_line"));
    let ft = |v: &serde_json::Value| v.as_f64().unwrap();
    assert!((ft(&got["entity"]["end"][1]) - 55.0).abs() < 1e-9);
    let listed = s.list(EntityKind::GridLine, None, 10);
    assert!((ft(&listed["rows"][1]["start"][0]) + 5.0).abs() < 1e-9);
    // A grid line with both ends at one point has no direction.
    let err = s
        .apply(vec![cmd(
            json!({"command": "update_grid_line", "id": a, "grid_line":
            {"name": "A", "start": [0, 0], "end": [0, 0]}}),
        )])
        .unwrap_err()
        .to_string();
    assert!(err.contains("two different ends"), "{err}");
}

#[test]
fn frames_carry_offsets_and_cardinal_points() {
    let mut s = Session::default();
    s.new_model("offsets").unwrap();
    let steel = s.add_material_from_library("A992", "steel").unwrap();
    let section = s.add_section_from_library("W14X90", "beam").unwrap();
    let base = s.find(EntityKind::Level, "Base").unwrap();
    let [a, b, beam, hung, dead] = <[_; 5]>::try_from(s.next_ids(5)).unwrap();
    s.apply(vec![
        cmd(
            json!({"command": "add_node", "id": a, "node": {"name": "A", "level": base,
            "position": [0, 0, 0], "restrained": [true, true, true, true, true, true]}}),
        ),
        cmd(
            json!({"command": "add_node", "id": b, "node": {"name": "B", "level": base,
            "position": [20, 0, 0], "restrained": [true, true, true, true, true, true]}}),
        ),
        cmd(
            json!({"command": "add_frame", "id": beam, "frame": {"name": "B1", "nodes": [a, b],
            "material": steel, "section": section, "local_y": [0, 0, 1],
            "offsets": {"end": [7, 7], "rigid_zone": 1}}}),
        ),
        cmd(
            json!({"command": "add_frame", "id": hung, "frame": {"name": "B2", "nodes": [a, b],
            "material": steel, "section": section, "local_y": [0, 0, 1],
            "cardinal_point": "top_center"}}),
        ),
        cmd(
            json!({"command": "add_load_case", "id": dead, "load_case": {"name": "dead",
            "self_weight": [0, 0, -1]}}),
        ),
    ])
    .unwrap();
    // Offsets read back in inches, and leave a plain frame's JSON alone.
    let got = &s.get(beam).unwrap()["entity"];
    assert!((got["offsets"]["end"][0].as_f64().unwrap() - 7.0).abs() < 1e-9);
    assert_eq!(got["offsets"]["rigid_zone"], json!(1.0));
    assert!(got.get("cardinal_point").is_none());
    assert_eq!(
        s.get(hung).unwrap()["entity"]["cardinal_point"],
        json!("top_center")
    );
    s.analyze(Default::default(), None).unwrap();
    let err = s
        .apply(vec![cmd(
            json!({"command": "update_frame", "id": beam, "frame": {"name": "B1",
            "nodes": [a, b], "material": steel, "section": section,
            "offsets": {"rigid_zone": 2}}}),
        )])
        .unwrap_err()
        .to_string();
    assert!(err.contains("rigid zone factor"), "{err}");
}

#[test]
fn replicate_copies_in_display_units_and_undoes() {
    let mut s = Session::default();
    s.new_model("replicate").unwrap();
    let steel = s.add_material_from_library("A992", "steel").unwrap();
    let section = s.add_section_from_library("W12x26", "beam").unwrap();
    let base = s.find(EntityKind::Level, "Base").unwrap();
    let [a, b, beam, dead] = <[_; 4]>::try_from(s.next_ids(4)).unwrap();
    s.apply(vec![
        cmd(json!({"command": "add_node", "id": a, "node": {"name": "N1", "level": base, "position": [0, 0, 0]}})),
        cmd(json!({"command": "add_node", "id": b, "node": {"name": "N2", "level": base, "position": [20, 0, 0]}})),
        cmd(json!({"command": "add_frame", "id": beam, "frame": {"name": "B1", "nodes": [a, b],
            "material": steel, "section": section}})),
        cmd(json!({"command": "add_load_case", "id": dead, "load_case": {"name": "D",
            "nodal": [{"node": b, "force": [2, 0, -1]}]}})),
    ])
    .unwrap();
    // Two 20 ft bays to the right of the first, sharing their ends.
    s.apply(vec![cmd(json!({"command": "replicate", "entities": [beam],
        "replication": {"type": "linear", "offset": [20, 0, 0], "count": 2}}))])
    .unwrap();
    let last = s.find(EntityKind::Frame, "B3").unwrap();
    let end = s.get(last).unwrap()["entity"]["nodes"][1].as_u64().unwrap();
    let position = s.get(oa_model::EntityId(end)).unwrap()["entity"]["position"].clone();
    assert!((position[0].as_f64().unwrap() - 60.0).abs() < 1e-9, "{position}");
    let loads = s.get(dead).unwrap()["entity"]["nodal"].clone();
    assert_eq!(loads.as_array().unwrap().len(), 3);
    // A quarter turn about the origin, in degrees: the load turns with it.
    s.apply(vec![cmd(json!({"command": "replicate", "entities": [b], "replication":
        {"type": "radial", "center": [0, 0], "angle": 90}}))])
    .unwrap();
    let loads = s.get(dead).unwrap()["entity"]["nodal"].clone();
    let turned = &loads.as_array().unwrap()[3]["force"];
    assert!(turned[0].as_f64().unwrap().abs() < 1e-9 && (turned[1].as_f64().unwrap() - 2.0).abs() < 1e-9, "{turned}");
    assert!(s.undo().unwrap());
    assert!(s.undo().unwrap());
    assert!(s.find(EntityKind::Frame, "B2").is_none());
}
