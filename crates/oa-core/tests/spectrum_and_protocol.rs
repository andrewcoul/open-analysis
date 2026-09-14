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
fn consumer_failure_stops_at_first_rejected_result() {
    struct Stop {
        calls: usize,
    }
    impl ResultConsumer for Stop {
        fn consume(&mut self, _: CombinationResult) -> Result<()> {
            self.calls += 1;
            Err(Error::Consumer("cancelled".into()))
        }
    }
    let AnalysisRequest::Static { model, .. } = request() else {
        unreachable!()
    };
    let mut stop = Stop { calls: 0 };
    let err = analyze_static_into(
        &model,
        &StaticOptions {
            max_in_flight: 1,
            ..Default::default()
        },
        &mut stop,
    )
    .unwrap_err();
    assert!(matches!(err, Error::Consumer(_)));
    assert_eq!(stop.calls, 1);
}
#[test]
fn envelopes_retain_combination_and_sign() {
    let envelope = Envelope::from_values([("a", -8.0), ("b", 3.0), ("c", -1.0)]).unwrap();
    assert_eq!(envelope.minimum.combination, "a");
    assert_eq!(envelope.maximum.combination, "b");
    assert_eq!(envelope.minimum.value, -8.0);
}
