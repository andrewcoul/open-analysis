use oa_core::{units::*, *};
fn request() -> AnalysisRequest {
    serde_json::from_str(include_str!("../../../examples/cantilever.json")).unwrap()
}
#[test]
fn single_mode_spectrum_matches_static_inertial_load() {
    let AnalysisRequest::Static { mut model, .. } = request() else {
        unreachable!()
    };
    model.materials[0].density = MassDensity::ZERO;
    model.nodes[1].mass = [Mass::ZERO, Mass::from_si(100.0), Mass::ZERO];
    let mut options = SpectrumOptions {
        modal: ModalOptions {
            modes: 1,
            ..Default::default()
        },
        direction: [0.0, 1.0, 0.0],
        spectrum: vec![
            SpectrumPoint {
                period_seconds: 0.0,
                acceleration: Acceleration::from_si(2.0),
            },
            SpectrumPoint {
                period_seconds: 10.0,
                acceleration: Acceleration::from_si(2.0),
            },
        ],
        minimum_mass_ratio: Some(0.99),
        ..Default::default()
    };
    for combination in [ModalCombination::Srss, ModalCombination::Cqc] {
        options.combination = combination;
        let r = analyze_spectrum(&model, &options).unwrap();
        let displacement = 100.0 * 2.0 * 27.0 / (3.0 * 200e9 * 4e-5);
        assert!((r.displacements.unwrap()[1][1] / displacement - 1.0).abs() < 1e-10);
        assert!((r.base_reaction[1] - 200.0).abs() < 1e-8);
        assert!((r.base_reaction[5] - 600.0).abs() < 1e-8);
    }
    options.spectrum[1].period_seconds = 0.0001;
    assert!(analyze_spectrum(&model, &options).is_err());
    options.direction = [0.0; 3];
    assert!(analyze_spectrum(&model, &options).is_err());
}
#[test]
fn versioned_json_round_trip_and_strict_validation() {
    let value = request();
    let json = serde_json::to_string(&value).unwrap();
    let response = solve_json(&json).unwrap();
    let roundtrip: AnalysisResponse = serde_json::from_str(&response).unwrap();
    assert!(matches!(
        roundtrip,
        AnalysisResponse::Static {
            schema_version: 1,
            ..
        }
    ));
    let mut bad: serde_json::Value = serde_json::from_str(&json).unwrap();
    bad["model"]["schema_version"] = serde_json::json!(99);
    assert!(solve_json(&bad.to_string()).is_err());
    bad["model"]["schema_version"] = serde_json::json!(1);
    bad["model"]["units"] = serde_json::json!("kips");
    assert!(solve_json(&bad.to_string()).is_err());
    assert!(solve_json("{broken").is_err());
}
#[test]
fn envelopes_retain_combination_and_sign() {
    let envelope = Envelope::from_values([("a", -8.0), ("b", 3.0), ("c", -1.0)]).unwrap();
    assert_eq!(envelope.minimum.combination, "a");
    assert_eq!(envelope.maximum.combination, "b");
    assert_eq!(envelope.minimum.value, -8.0);
}

#[test]
fn spectrum_base_shear_includes_forces_transferred_to_a_restrained_master() {
    // Cantilever along X; its tip is a slave of a Z-normal diaphragm whose
    // master at (3, 1, 0) is restrained in X only. Tip mass in X moves only
    // through the master's rotation, and the inertial force reaches the
    // supports partly through the beam and partly through the master.
    let mut model = Model::default();
    model.add_material(Material {
        young: Pressure::from_si(200e9),
        poisson: 0.3,
        density: MassDensity::ZERO,
    });
    model.add_section(Section {
        area: Area::from_si(0.01),
        iy: SecondMoment::from_si(2e-5),
        iz: SecondMoment::from_si(4e-5),
        torsion: SecondMoment::from_si(1e-5),
    });
    model.add_node(Node::fixed([Length::ZERO; 3]));
    let mut tip = Node::new([Length::from_si(3.0), Length::ZERO, Length::ZERO]);
    tip.mass[0] = Mass::from_si(100.0);
    model.add_node(tip);
    let mut master = Node::new([Length::from_si(3.0), Length::from_si(1.0), Length::ZERO]);
    master.restrained[0] = true;
    model.add_node(master);
    model.add_frame(Frame::new(
        [NodeId(0), NodeId(1)],
        MaterialId(0),
        SectionId(0),
    ));
    model.diaphragms.push(Diaphragm {
        master: NodeId(2),
        nodes: vec![NodeId(1)],
        normal: Axis::Z,
    });
    model.add_load_case(LoadCase {
        name: "none".into(),
        ..Default::default()
    });
    let options = SpectrumOptions {
        modal: ModalOptions {
            modes: 1,
            ..Default::default()
        },
        direction: [1.0, 0.0, 0.0],
        spectrum: vec![
            SpectrumPoint {
                period_seconds: 0.0,
                acceleration: Acceleration::from_si(2.0),
            },
            SpectrumPoint {
                period_seconds: 10.0,
                acceleration: Acceleration::from_si(2.0),
            },
        ],
        minimum_mass_ratio: Some(0.99),
        ..Default::default()
    };
    let r = analyze_spectrum(&model, &options).unwrap();
    let reactions = r.reactions.as_ref().unwrap();
    // The tip moves in X only through the master's rotation, which the beam
    // resists axially (EA/L, reaching the base) and in bending with a free tip
    // translation (EI/L, reaching the master). The shear splits in that ratio.
    let axial = 200e9 * 0.01 / 3.0;
    let bending = 200e9 * 4e-5 / 3.0;
    let base_share = 200.0 * axial / (axial + bending);
    let master_share = 200.0 * bending / (axial + bending);
    assert!((reactions[0][0] - base_share).abs() < 1e-8 * base_share);
    assert!((reactions[2][0] - master_share).abs() < 1e-8 * master_share);
    // Base shear is the whole inertial force, mass times spectral acceleration.
    assert!((r.base_reaction[0] - 200.0).abs() < 1e-8);
    // The inertial force acts along the X axis through the origin, so the
    // reaction moments about the origin cancel.
    assert!(r.base_reaction[5].abs() < 1e-8);
}
