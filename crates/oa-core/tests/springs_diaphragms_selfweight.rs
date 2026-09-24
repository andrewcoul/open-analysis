use oa_core::{units::*, *};

const E: f64 = 200e9;
const NU: f64 = 0.3;
const IZ: f64 = 4e-5;
const AREA: f64 = 0.01;
const J: f64 = 1e-5;

fn close(actual: f64, expected: f64, tol: f64) {
    assert!(
        (actual - expected).abs() <= tol * expected.abs().max(1e-12),
        "actual {actual:e}, expected {expected:e}"
    );
}
fn base(density: f64) -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_si(E),
        poisson: NU,
        density: MassDensity::from_si(density),
    });
    m.add_section(Section {
        area: Area::from_si(AREA),
        iy: SecondMoment::from_si(2e-5),
        iz: SecondMoment::from_si(IZ),
        torsion: SecondMoment::from_si(J),
    });
    m
}
fn cantilever(density: f64) -> Model {
    let mut m = base(density);
    m.add_node(Node::fixed([Length::ZERO; 3]));
    m.add_node(Node::new([
        Length::from_si(3.0),
        Length::ZERO,
        Length::ZERO,
    ]));
    m.add_frame(Frame::new(
        [NodeId(0), NodeId(1)],
        MaterialId(0),
        SectionId(0),
    ));
    m
}

#[test]
fn self_weight_matches_hand_calc_and_explicit_line_load() {
    let density = 7850.0;
    let w = density * AREA * STANDARD_GRAVITY;
    let mut m = cantilever(density);
    m.add_load_case(LoadCase {
        name: "sw".into(),
        self_weight: [0.0, -1.0, 0.0],
        ..Default::default()
    });
    let r = analyze_static(&m, &Default::default()).unwrap();
    let c = &r.combinations[0];
    close(
        c.displacements.as_ref().unwrap()[1][1],
        -w * 81.0 / (8.0 * E * IZ),
        1e-10,
    );
    close(c.reactions.as_ref().unwrap()[0][1], w * 3.0, 1e-10);
    close(c.reactions.as_ref().unwrap()[0][5], w * 4.5, 1e-10);
    let section = frame_section_forces(
        &m,
        &m.effective_combinations()[0],
        FrameId(0),
        &c.frames.as_ref().unwrap()[0],
        Length::from_si(1.0),
    )
    .unwrap();
    close(section.values[1], -w * 2.0, 1e-10);
    close(section.values[5], -w * 2.0, 1e-10);

    let mut explicit = cantilever(density);
    explicit.add_load_case(LoadCase {
        name: "udl".into(),
        member: vec![MemberLoad::Distributed {
            member: FrameId(0),
            start: Length::ZERO,
            end: Length::from_si(3.0),
            start_load: [LineLoad::ZERO, LineLoad::from_si(-w), LineLoad::ZERO],
            end_load: [LineLoad::ZERO, LineLoad::from_si(-w), LineLoad::ZERO],
            axes: Axes::Global,
        }],
        ..Default::default()
    });
    let e = analyze_static(&explicit, &Default::default()).unwrap();
    for (a, b) in c.displacements.as_ref().unwrap().iter().flatten().zip(
        e.combinations[0]
            .displacements
            .as_ref()
            .unwrap()
            .iter()
            .flatten(),
    ) {
        close(*a, *b, 1e-12);
    }
}

#[test]
fn shell_self_weight_totals_density_times_volume() {
    let n = 4;
    let density = 2400.0;
    let thickness = 0.2;
    let mut m = base(density);
    for j in 0..=n {
        for i in 0..=n {
            let mut node = Node::new([
                Length::from_si(i as f64 / n as f64),
                Length::from_si(j as f64 / n as f64),
                Length::ZERO,
            ]);
            node.restrained = [
                true,
                true,
                i == 0 || i == n || j == 0 || j == n,
                false,
                false,
                true,
            ];
            m.add_node(node);
        }
    }
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            m.add_shell(Shell {
                nodes: [
                    NodeId(a),
                    NodeId(a + 1),
                    NodeId(a + n + 2),
                    NodeId(a + n + 1),
                ],
                material: MaterialId(0),
                thickness: Length::from_si(thickness),
                formulation: ShellFormulation::Dkmq,
                drilling_ratio: 1e-3,
            });
        }
    }
    m.add_load_case(LoadCase {
        name: "sw".into(),
        self_weight: [0.0, 0.0, -1.0],
        ..Default::default()
    });
    let r = analyze_static(&m, &Default::default()).unwrap();
    let total: f64 = r.combinations[0]
        .reactions
        .as_ref()
        .unwrap()
        .iter()
        .map(|v| v[2])
        .sum();
    close(total, density * thickness * STANDARD_GRAVITY, 1e-9);
}

#[test]
fn tip_spring_shares_load_with_beam_and_reports_reaction() {
    let ks = 1e6;
    let kb = 3.0 * E * IZ / 27.0;
    let p = -1000.0;
    let mut m = cantilever(0.0);
    m.nodes[1].spring_translation[1] = Stiffness::from_si(ks);
    m.add_load_case(LoadCase {
        name: "tip".into(),
        nodal: vec![NodalLoad::force(
            NodeId(1),
            [Force::ZERO, Force::from_si(p), Force::ZERO],
        )],
        ..Default::default()
    });
    let r = analyze_static(&m, &Default::default()).unwrap();
    let c = &r.combinations[0];
    let u = p / (kb + ks);
    close(c.displacements.as_ref().unwrap()[1][1], u, 1e-10);
    let reactions = c.reactions.as_ref().unwrap();
    close(reactions[1][1], -ks * u, 1e-10);
    close(reactions[0][1], -p - reactions[1][1], 1e-10);

    // Springs are stiffness, so they shift modal frequencies too.
    m.nodes[1].mass[1] = Mass::from_si(50.0);
    let modal = analyze_modal(
        &m,
        &ModalOptions {
            modes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    close(modal.modes[0].eigenvalue, (kb + ks) / 50.0, 1e-10);
}

/// Two columns at z = -e and z = +e under a Y-normal diaphragm whose master
/// sits midway. In-plane rotation of the master twists both columns.
fn two_column_diaphragm(e: f64) -> Model {
    let h = 3.0;
    let mut m = base(0.0);
    for z in [-e, e] {
        m.add_node(Node::fixed([
            Length::ZERO,
            Length::ZERO,
            Length::from_si(z),
        ]));
        m.add_node(Node::new([
            Length::ZERO,
            Length::from_si(h),
            Length::from_si(z),
        ]));
    }
    m.add_node(Node::new([Length::ZERO, Length::from_si(h), Length::ZERO]));
    m.add_frame(Frame::new(
        [NodeId(0), NodeId(1)],
        MaterialId(0),
        SectionId(0),
    ));
    m.add_frame(Frame::new(
        [NodeId(2), NodeId(3)],
        MaterialId(0),
        SectionId(0),
    ));
    m.diaphragms.push(Diaphragm {
        master: NodeId(4),
        nodes: vec![NodeId(1), NodeId(3)],
        normal: Axis::Y,
    });
    m
}
fn column_stiffness() -> (f64, f64) {
    let h: f64 = 3.0;
    let lateral = 3.0 * E * IZ / h.powi(3);
    let torsion = E / (2.0 * (1.0 + NU)) * J / h;
    (lateral, torsion)
}

#[test]
fn diaphragm_eccentric_load_twists_and_shares_by_lever_arm() {
    let e = 2.0;
    let p = 1000.0;
    let (k, kt) = column_stiffness();
    let mut m = two_column_diaphragm(e);
    m.add_load_case(LoadCase {
        name: "eccentric".into(),
        nodal: vec![NodalLoad::force(
            NodeId(1),
            [Force::from_si(p), Force::ZERO, Force::ZERO],
        )],
        ..Default::default()
    });
    let r = analyze_static(&m, &Default::default()).unwrap();
    let c = &r.combinations[0];
    let u = c.displacements.as_ref().unwrap();
    // Reduced system on (u_x, theta_y): K = diag(2k, 2(k e^2 + kt)), f = (P, -P e).
    let um = p / (2.0 * k);
    let theta = -p * e / (2.0 * (k * e * e + kt));
    close(u[4][0], um, 1e-9);
    close(u[4][4], theta, 1e-9);
    close(u[1][0], um - e * theta, 1e-9);
    close(u[3][0], um + e * theta, 1e-9);
    close(u[1][4], theta, 1e-9);
    let reactions = c.reactions.as_ref().unwrap();
    close(reactions[0][0] + reactions[2][0], -p, 1e-9);
    close(reactions[0][0], -k * u[1][0], 1e-9);
    // The master's unstiffened out-of-plane DOFs are dropped, not reported unstable,
    // and a load on one of them is rejected rather than silently lost.
    m.load_cases[0].nodal[0] =
        NodalLoad::force(NodeId(4), [Force::ZERO, Force::from_si(1.0), Force::ZERO]);
    assert!(matches!(
        analyze_static(&m, &Default::default()),
        Err(Error::Unstable(_))
    ));
}

#[test]
fn diaphragm_modal_couples_offset_masses_exactly() {
    let e = 2.0;
    let (m1, m2) = (100.0, 300.0);
    let (k, kt) = column_stiffness();
    let mut m = two_column_diaphragm(e);
    m.nodes[1].mass[0] = Mass::from_si(m1);
    m.nodes[3].mass[0] = Mass::from_si(m2);
    let r = analyze_modal(
        &m,
        &ModalOptions {
            modes: 2,
            ..Default::default()
        },
    )
    .unwrap();
    // Reduced K = diag(2k, 2(k e^2 + kt)); reduced M = Tᵀ M T with lever arms -e, +e.
    let (k11, k22) = (2.0 * k, 2.0 * (k * e * e + kt));
    let (mm11, mm12, mm22) = (m1 + m2, (m2 - m1) * e, (m1 + m2) * e * e);
    // det(K - lambda M) = 0 is a quadratic in lambda.
    let a = mm11 * mm22 - mm12 * mm12;
    let b = -(k11 * mm22 + k22 * mm11);
    let c = k11 * k22;
    let disc = (b * b - 4.0 * a * c).sqrt();
    let expected = [(-b - disc) / (2.0 * a), (-b + disc) / (2.0 * a)];
    close(r.modes[0].eigenvalue, expected[0], 1e-9);
    close(r.modes[1].eigenvalue, expected[1], 1e-9);
    close(r.cumulative_mass_ratio[0], 1.0, 1e-9);
    assert!(r.maximum_mass_orthogonality_error < 1e-9);
    assert_eq!(r.sturm_count_below_cutoff, Some(1));
    assert_eq!(
        modal_inertia_count(&m, r.modes[1].frequency_hz * 1.01).unwrap(),
        2
    );
    // A master away from the centre of mass would be wrong under simple lumping.
    let lumped = 2.0 * k / (m1 + m2);
    assert!((r.modes[0].eigenvalue / lumped - 1.0).abs() > 0.05);
}

#[test]
fn rejects_conflicting_diaphragms_and_springs() {
    let mut m = two_column_diaphragm(2.0);
    m.add_load_case(LoadCase {
        name: "a".into(),
        ..Default::default()
    });
    assert!(m.validate().is_ok());
    let mut twice = m.clone();
    twice.diaphragms.push(Diaphragm {
        master: NodeId(0),
        nodes: vec![NodeId(1)],
        normal: Axis::Y,
    });
    assert!(twice.validate().is_err());
    let mut restrained_slave = m.clone();
    restrained_slave.nodes[1].restrained[0] = true;
    assert!(restrained_slave.validate().is_err());
    let mut sprung_slave = m.clone();
    sprung_slave.nodes[1].spring_rotation[1] = RotationalStiffness::from_si(1.0);
    assert!(sprung_slave.validate().is_err());
    let mut sprung_restraint = m.clone();
    sprung_restraint.nodes[0].spring_translation[0] = Stiffness::from_si(1.0);
    assert!(sprung_restraint.validate().is_err());
    m.nodes[1].restrained[1] = true;
    assert!(
        m.validate().is_ok(),
        "out-of-plane restraint on a slave is fine"
    );
}

#[test]
fn nonlinear_methods_include_spring_forces_in_equilibrium() {
    // A grounded spring is part of the solved stiffness, so the iterative
    // methods must count its force as internal. Both must reproduce the
    // linear answer for this elastic configuration in one pass.
    let ks = 1e6;
    let kb = 3.0 * E * IZ / 27.0;
    let p = -1000.0;
    let mut m = cantilever(0.0);
    m.nodes[1].spring_translation[1] = Stiffness::from_si(ks);
    m.add_load_case(LoadCase {
        name: "tip".into(),
        nodal: vec![NodalLoad::force(
            NodeId(1),
            [Force::ZERO, Force::from_si(p), Force::ZERO],
        )],
        ..Default::default()
    });
    let u = p / (kb + ks);
    for method in [StaticMethod::Nonlinear, StaticMethod::PDelta] {
        let r = analyze_static(
            &m,
            &StaticOptions {
                method,
                max_iterations: 3,
                ..Default::default()
            },
        )
        .unwrap();
        let c = &r.combinations[0];
        close(c.displacements.as_ref().unwrap()[1][1], u, 1e-10);
        close(c.reactions.as_ref().unwrap()[1][1], -ks * u, 1e-10);
        assert!(
            c.relative_residual < 1e-9,
            "{method:?}: {}",
            c.relative_residual
        );
    }

    // The same holds for a spring on a diaphragm master.
    let (k, _) = column_stiffness();
    let mut d = two_column_diaphragm(2.0);
    d.nodes[4].spring_translation[0] = Stiffness::from_si(ks);
    d.add_load_case(LoadCase {
        name: "master".into(),
        nodal: vec![NodalLoad::force(
            NodeId(4),
            [Force::from_si(1000.0), Force::ZERO, Force::ZERO],
        )],
        ..Default::default()
    });
    let um = 1000.0 / (2.0 * k + ks);
    for method in [
        StaticMethod::Linear,
        StaticMethod::Nonlinear,
        StaticMethod::PDelta,
    ] {
        let r = analyze_static(
            &d,
            &StaticOptions {
                method,
                max_iterations: 3,
                ..Default::default()
            },
        )
        .unwrap();
        let c = &r.combinations[0];
        close(c.displacements.as_ref().unwrap()[4][0], um, 1e-9);
        close(c.reactions.as_ref().unwrap()[4][0], -ks * um, 1e-9);
    }
}

/// Cantilever along X whose tip is a slave of a Z-normal diaphragm with an
/// offset master at (3, 1, 0). A tip load in X passes through the constraint.
fn offset_master_cantilever(master_restrained: [bool; 6]) -> Model {
    let mut m = cantilever(0.0);
    let mut master = Node::new([Length::from_si(3.0), Length::from_si(1.0), Length::ZERO]);
    master.restrained = master_restrained;
    m.add_node(master);
    m.diaphragms.push(Diaphragm {
        master: NodeId(2),
        nodes: vec![NodeId(1)],
        normal: Axis::Z,
    });
    m.add_load_case(LoadCase {
        name: "tip x".into(),
        nodal: vec![NodalLoad::force(
            NodeId(1),
            [Force::from_si(1000.0), Force::ZERO, Force::ZERO],
        )],
        ..Default::default()
    });
    m
}

#[test]
fn restrained_diaphragm_master_reports_transferred_reactions() {
    let m = offset_master_cantilever([true; 6]);
    let r = analyze_static(&m, &Default::default()).unwrap();
    let c = &r.combinations[0];
    let reactions = c.reactions.as_ref().unwrap();
    let rx: f64 = reactions.iter().map(|v| v[0]).sum();
    close(rx, -1000.0, 1e-12);
    // The beam carries no axial force: the whole load reaches the master,
    // together with the moment of its 1 m lever arm.
    assert!(reactions[0][0].abs() < 1e-9);
    close(reactions[2][0], -1000.0, 1e-12);
    close(reactions[2][5], -1000.0, 1e-12);
    assert!(reactions[1].iter().all(|v| *v == 0.0), "slaves report none");
    assert!(c.relative_residual < 1e-10);
}

#[test]
fn partially_restrained_master_balances_forces_and_moments() {
    // Only the master's X translation is restrained. Its rotation is free, so
    // part of the load reaches the base through the beam and part through the
    // master; together they must balance the applied load and its moment.
    let m = offset_master_cantilever([true, false, false, false, false, false]);
    let r = analyze_static(&m, &Default::default()).unwrap();
    let c = &r.combinations[0];
    let reactions = c.reactions.as_ref().unwrap();
    let base = reactions[0];
    let master = reactions[2];
    assert!(master[0].abs() > 1.0, "master carries part of the load");
    assert!(base[0].abs() > 1.0, "the beam carries the rest axially");
    close(base[0] + master[0], -1000.0, 1e-10);
    assert!(base[1].abs() < 1e-9 * 1000.0);
    // Moments about the origin: the load acts along its own line through the
    // origin, so the reaction moments must cancel; the master's force at
    // (3, 1, 0) contributes -1 m * Rx.
    let moment = base[5] + (3.0 * master[1] - 1.0 * master[0]);
    assert!(moment.abs() < 1e-9 * 1000.0, "moment imbalance {moment}");
    assert!(
        master[1..].iter().all(|v| *v == 0.0),
        "unrestrained master DOFs report none"
    );
}
