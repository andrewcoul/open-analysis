//! The Phase M6 acceptance scenario, driven through the session exactly as
//! the MCP tools would: build a two-storey frame from commands, run it, and
//! report the governing drift. No hand-written solver code.
use oa_mcp::{Quantity, Session};
use oa_model::EntityKind;
use serde_json::json;

fn cmd(v: serde_json::Value) -> oa_model::Command {
    serde_json::from_value(v).unwrap()
}

#[test]
fn agent_scenario_two_storey_frame_to_governing_drift() {
    let mut s = Session::default();
    s.new_model("two storey");
    let steel = s.add_material_from_library("A992", "steel").unwrap();
    let column = s.add_section_from_library("W14x90", "column").unwrap();
    let beam = s.add_section_from_library("W12x26", "beam").unwrap();

    // Six nodes: two fixed at grade, two per floor.
    let ids = s.next_ids(6);
    let mut commands = vec![];
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
        commands.push(cmd(json!({
            "command": "add_node", "id": ids[i],
            "node": {"name": format!("N{i}"), "position": [x, y, 0.0], "restrained": vec![i < 2; 6]}
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
        "name": "dead", "self_weight": [0, -1, 0],
        "member": [{"type": "distributed", "member": frames[4], "start": 0, "end": 6,
                    "start_load": [0, -15000, 0], "end_load": [0, -15000, 0], "axes": "global"},
                   {"type": "distributed", "member": frames[5], "start": 0, "end": 6,
                    "start_load": [0, -10000, 0], "end_load": [0, -10000, 0], "axes": "global"}]}}),
    ));
    commands.push(cmd(json!({"command": "add_load_case", "id": wind, "load_case": {
        "name": "wind", "nodal": [{"node": ids[2], "force": [12000, 0, 0]}, {"node": ids[4], "force": [8000, 0, 0]}]}})));
    commands.push(cmd(
        json!({"command": "add_combination", "id": combo, "combination": {
        "name": "1.2D+1.0W", "terms": [[dead, 1.2], [wind, 1.0]]}}),
    ));
    commands.push(cmd(json!({"command": "add_group", "id": roof, "group": {"name": "roof", "members": [ids[4], ids[5], frames[5]]}})));
    let applied = s.apply(commands).unwrap();
    assert_eq!(applied["applied"], 16);
    assert_eq!(s.describe()["counts"]["frames"], 6);

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
    let moments = s
        .group_envelope(roof, Quantity::FrameForce, "mz_j", 10)
        .unwrap();
    assert_eq!(
        moments["total"], 1,
        "only the roof beam is a frame in the group"
    );
    let base = s.envelope(ids[0], Quantity::Reaction, "uy").unwrap();
    assert!(
        base["maximum"]["value"].as_f64().unwrap() > 0.0,
        "base carries gravity"
    );
    let table = s
        .query("select count(*) as n from frame_results", 10)
        .unwrap();
    assert_eq!(table["rows"][0][0], 6);
    let indices = s.entity_indices(&[ids[4], frames[5]]).unwrap();
    assert!(indices[0]["node"].is_number() && indices[1]["frame"].is_number());

    // Edits discard results, and undo restores the previous state.
    let extra = s.next_ids(1)[0];
    s.apply(vec![cmd(
        json!({"command": "add_node", "id": extra, "node": {"name": "N6", "position": [3, 8, 0]}}),
    )])
    .unwrap();
    assert!(s.envelope(ids[4], Quantity::Displacement, "ux").is_err());
    assert!(s.undo().unwrap());
    assert_eq!(s.describe()["counts"]["nodes"], 6);
    assert_eq!(s.find(EntityKind::Frame, "B2"), Some(frames[5]));
    let listed = s.list(EntityKind::Frame, Some("C"), 2);
    assert_eq!(listed["total"], 4);
    assert_eq!(listed["truncated"], true);

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
    s.new_model("saved");
    let steel = s.add_material_from_library("S355", "steel").unwrap();
    assert!(steel.0 > 0);
    let saved = s.save(Some(path.clone())).unwrap();
    assert_eq!(saved, path);
    let mut t = Session::default();
    let described = t.load(path).unwrap();
    assert_eq!(described["name"], "saved");
    assert_eq!(described["counts"]["materials"], 1);
    let _ = std::fs::remove_dir_all(&dir);
}
