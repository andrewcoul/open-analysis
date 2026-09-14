use oa_core::{units::*, *};

fn close(actual: f64, expected: f64, tol: f64) {
    assert!(
        (actual - expected).abs() <= tol * expected.abs().max(1e-12),
        "actual {actual:e}, expected {expected:e}"
    );
}
fn base() -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_gpa(200.0),
        poisson: 0.3,
        density: MassDensity::ZERO,
    });
    m.add_section(Section {
        area: Area::from_si(0.01),
        iy: SecondMoment::from_si(2e-5),
        iz: SecondMoment::from_si(4e-5),
        torsion: SecondMoment::from_si(1e-5),
    });
    m
}
fn cantilever(segments: usize) -> Model {
    let mut m = base();
    for i in 0..=segments {
        let p = [
            Length::from_si(3.0 * i as f64 / segments as f64),
            Length::ZERO,
            Length::ZERO,
        ];
        m.add_node(if i == 0 { Node::fixed(p) } else { Node::new(p) });
        if i > 0 {
            m.add_frame(Frame::new(
                [NodeId(i - 1), NodeId(i)],
                MaterialId(0),
                SectionId(0),
            ));
        }
    }
    m
}
#[test]
fn cantilever_all_six_dofs_and_equilibrium() {
    let mut m = cantilever(1);
    m.add_load_case(LoadCase {
        name: "tip".into(),
        nodal: vec![NodalLoad {
            node: NodeId(1),
            force: [
                Force::from_si(1000.0),
                Force::from_si(-2000.0),
                Force::from_si(3000.0),
            ],
            moment: [
                Moment::from_si(500.0),
                Moment::from_si(600.0),
                Moment::from_si(-700.0),
            ],
        }],
        ..Default::default()
    });
    let r = analyze_static(&m, &StaticOptions::default()).unwrap();
    let c = &r.combinations[0];
    let u = c.displacements.as_ref().unwrap()[1];
    let rx = c.reactions.as_ref().unwrap()[0];
    close(u[0], 1000.0 * 3.0 / (200e9 * 0.01), 1e-10);
    close(
        u[1],
        -2000.0 * 27.0 / (3.0 * 200e9 * 4e-5) - 700.0 * 9.0 / (2.0 * 200e9 * 4e-5),
        1e-10,
    );
    close(
        u[2],
        3000.0 * 27.0 / (3.0 * 200e9 * 2e-5) - 600.0 * 9.0 / (2.0 * 200e9 * 2e-5),
        1e-10,
    );
    close(u[3], 500.0 * 3.0 / (200e9 / 2.6 * 1e-5), 1e-10);
    close(rx[0], -1000.0, 1e-10);
    close(rx[1], 2000.0, 1e-10);
    close(rx[2], -3000.0, 1e-10);
    close(rx[4], 8400.0, 1e-10);
    close(rx[5], 6700.0, 1e-10);
}
#[test]
fn distributed_cantilever_and_exact_diagram() {
    let mut m = cantilever(1);
    m.add_load_case(LoadCase {
        name: "udl".into(),
        member: vec![MemberLoad::Distributed {
            member: FrameId(0),
            start: Length::ZERO,
            end: Length::from_si(3.0),
            start_load: [LineLoad::ZERO, LineLoad::from_si(-1000.0), LineLoad::ZERO],
            end_load: [LineLoad::ZERO, LineLoad::from_si(-1000.0), LineLoad::ZERO],
            axes: Axes::Local,
        }],
        ..Default::default()
    });
    let r = analyze_static(&m, &StaticOptions::default()).unwrap();
    let c = &r.combinations[0];
    let u = c.displacements.as_ref().unwrap()[1];
    close(u[1], -1000.0 * 81.0 / (8.0 * 200e9 * 4e-5), 1e-10);
    let f = frame_section_forces(
        &m,
        &m.effective_combinations()[0],
        FrameId(0),
        &c.frames.as_ref().unwrap()[0],
        Length::from_si(1.0),
    )
    .unwrap();
    close(f.values[1], -2000.0, 1e-10);
    close(f.values[5], -2000.0, 1e-10);
}
#[test]
fn released_fixed_fixed_beam_becomes_simply_supported() {
    let mut m = cantilever(1);
    m.nodes[1].restrained = [true; 6];
    m.frames[0].releases[5] = true;
    m.frames[0].releases[11] = true;
    m.add_load_case(LoadCase {
        name: "udl".into(),
        member: vec![MemberLoad::Distributed {
            member: FrameId(0),
            start: Length::ZERO,
            end: Length::from_si(3.0),
            start_load: [LineLoad::ZERO, LineLoad::from_si(-1000.0), LineLoad::ZERO],
            end_load: [LineLoad::ZERO, LineLoad::from_si(-1000.0), LineLoad::ZERO],
            axes: Axes::Local,
        }],
        ..Default::default()
    });
    let r = analyze_static(&m, &StaticOptions::default()).unwrap();
    let c = &r.combinations[0];
    let f = &c.frames.as_ref().unwrap()[0];
    close(c.reactions.as_ref().unwrap()[0][1], 1500.0, 1e-10);
    assert_eq!(f.local_end_forces[5], 0.0);
    assert_eq!(f.local_end_forces[11], 0.0);
    close(
        f.local_displacements[5],
        -1000.0 * 27.0 / (24.0 * 200e9 * 4e-5),
        1e-10,
    );
    close(
        frame_section_forces(
            &m,
            &m.effective_combinations()[0],
            FrameId(0),
            f,
            Length::from_si(1.5),
        )
        .unwrap()
        .values[5],
        1125.0,
        1e-10,
    );
}
#[test]
fn load_combinations_parallel_order_and_output_selection() {
    let mut m = cantilever(1);
    let id = m.add_load_case(LoadCase {
        name: "a".into(),
        nodal: vec![NodalLoad::force(
            NodeId(1),
            [Force::ZERO, Force::from_si(-1000.0), Force::ZERO],
        )],
        ..Default::default()
    });
    for i in 0..17 {
        m.combinations.push(LoadCombination {
            name: format!("c{i}"),
            terms: vec![(id, i as f64 - 8.0)],
        });
    }
    let a = analyze_static(
        &m,
        &StaticOptions {
            threads: 1,
            max_in_flight: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let b = analyze_static(
        &m,
        &StaticOptions {
            threads: 4,
            max_in_flight: 3,
            ..Default::default()
        },
    )
    .unwrap();
    for i in 0..17 {
        assert_eq!(b.combinations[i].combination, format!("c{i}"));
        // Multithreaded factorization changes summation order, so agreement
        // is to roundoff rather than bit-for-bit.
        let (ua, ub) = (
            a.combinations[i].displacements.as_ref().unwrap(),
            b.combinations[i].displacements.as_ref().unwrap(),
        );
        let scale = ua.iter().flatten().fold(0.0_f64, |m, v| m.max(v.abs()));
        for (x, y) in ua.iter().flatten().zip(ub.iter().flatten()) {
            assert!((x - y).abs() <= 1e-12 * scale, "{x:e} vs {y:e}");
        }
    }
    let c = analyze_static(
        &m,
        &StaticOptions {
            combinations: vec!["c3".into()],
            outputs: OutputSelection {
                frames: false,
                shells: false,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(c.combinations.len(), 1);
    assert!(c.combinations[0].frames.is_none());
}
#[test]
fn prescribed_settlement_and_no_external_load() {
    let mut m = cantilever(1);
    m.nodes[1].restrained = [true; 6];
    m.nodes[1].prescribed.translation[0] = Length::from_si(0.001);
    m.add_load_case(LoadCase {
        name: "settlement".into(),
        ..Default::default()
    });
    let r = analyze_static(&m, &StaticOptions::default()).unwrap();
    close(
        r.combinations[0].reactions.as_ref().unwrap()[1][0],
        200e9 * 0.01 / 3.0 * 0.001,
        1e-10,
    );
}
#[test]
fn unilateral_state_is_independent_per_combination() {
    let mut m = cantilever(1);
    let mut brace = m.frames[0].clone();
    brace.behavior = AxialBehavior::TensionOnly;
    m.add_frame(brace);
    let id = m.add_load_case(LoadCase {
        name: "axial".into(),
        nodal: vec![NodalLoad::force(
            NodeId(1),
            [Force::from_si(1000.0), Force::ZERO, Force::ZERO],
        )],
        ..Default::default()
    });
    m.combinations = vec![
        LoadCombination {
            name: "tension".into(),
            terms: vec![(id, 1.0)],
        },
        LoadCombination {
            name: "compression".into(),
            terms: vec![(id, -1.0)],
        },
    ];
    let r = analyze_static(
        &m,
        &StaticOptions {
            method: StaticMethod::Nonlinear,
            ..Default::default()
        },
    )
    .unwrap();
    close(
        r.combinations[0].displacements.as_ref().unwrap()[1][0],
        1000.0 * 3.0 / (2.0 * 200e9 * 0.01),
        1e-9,
    );
    close(
        r.combinations[1].displacements.as_ref().unwrap()[1][0],
        -1000.0 * 3.0 / (200e9 * 0.01),
        1e-9,
    );
    assert!(r.combinations[0].frames.as_ref().unwrap()[1].active);
    assert!(!r.combinations[1].frames.as_ref().unwrap()[1].active);
}
#[test]
fn p_delta_cantilever_against_closed_form() {
    let mut m = cantilever(12);
    let p = 300000.0;
    let h = 1000.0;
    m.add_load_case(LoadCase {
        name: "column".into(),
        nodal: vec![NodalLoad::force(
            NodeId(12),
            [Force::from_si(-p), Force::from_si(h), Force::ZERO],
        )],
        ..Default::default()
    });
    let r = analyze_static(
        &m,
        &StaticOptions {
            method: StaticMethod::PDelta,
            ..Default::default()
        },
    )
    .unwrap();
    let k = (p / (200e9 * 4e-5)).sqrt();
    let expected = h / p * ((k * 3.0).tan() / k - 3.0);
    close(
        r.combinations[0].displacements.as_ref().unwrap()[12][1],
        expected,
        2e-5,
    );
    close(
        r.combinations[0].reactions.as_ref().unwrap()[0][5],
        -(h * 3.0 + p * expected),
        2e-5,
    );
}
#[test]
fn rejects_mechanism_bad_ids_and_nonfinite_input() {
    let mut m = cantilever(1);
    m.nodes[0].restrained = [false; 6];
    m.add_load_case(LoadCase {
        name: "a".into(),
        ..Default::default()
    });
    assert!(matches!(
        analyze_static(&m, &StaticOptions::default()),
        Err(Error::Unstable(_))
    ));
    m.frames[0].nodes[0] = NodeId(99);
    assert!(m.validate().is_err());
    m.frames[0].nodes[0] = NodeId(0);
    m.materials[0].young = Pressure::from_si(f64::NAN);
    assert!(m.validate().is_err());
}
#[test]
fn equivalent_imperial_inputs_and_mass_weight_conversion() {
    let mut metric = cantilever(1);
    metric.nodes[1].position[0] = Length::from_si(3.048);
    metric.materials[0].young = Pressure::from_si(Pressure::from_ksi(29000.0).si());
    metric.sections[0].area = Area::from_si(Area::from_square_inches(10.0).si());
    let mut imperial = metric.clone();
    imperial.nodes[1].position[0] = Length::from_feet(10.0);
    imperial.materials[0].young = Pressure::from_ksi(29000.0);
    imperial.sections[0].area = Area::from_square_inches(10.0);
    metric.add_load_case(LoadCase {
        name: "a".into(),
        nodal: vec![NodalLoad::force(
            NodeId(1),
            [Force::from_si(4448.2216152605), Force::ZERO, Force::ZERO],
        )],
        ..Default::default()
    });
    imperial.add_load_case(LoadCase {
        name: "a".into(),
        nodal: vec![NodalLoad::force(
            NodeId(1),
            [Force::from_kips(1.0), Force::ZERO, Force::ZERO],
        )],
        ..Default::default()
    });
    assert_eq!(
        analyze_static(&metric, &Default::default())
            .unwrap()
            .combinations[0]
            .displacements,
        analyze_static(&imperial, &Default::default())
            .unwrap()
            .combinations[0]
            .displacements
    );
    close(
        WeightDensity::from_lbf_per_ft3(490.0)
            .to_mass_density(Acceleration::from_si(9.80665))
            .unwrap()
            .si(),
        MassDensity::from_lbm_per_ft3(490.0).si(),
        1e-12,
    );
}
