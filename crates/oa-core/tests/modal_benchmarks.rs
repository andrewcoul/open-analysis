use oa_core::{units::*, *};
fn independent_cantilevers(n: usize, split: f64) -> Model {
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
    for i in 0..n {
        let a = m.add_node(Node::fixed([
            Length::ZERO,
            Length::from_si(i as f64),
            Length::ZERO,
        ]));
        let mut node = Node::new([
            Length::from_si(3.0),
            Length::from_si(i as f64),
            Length::ZERO,
        ]);
        node.mass[1] = Mass::from_si(100.0 * (1.0 + i as f64 * split));
        let b = m.add_node(node);
        m.add_frame(Frame::new([a, b], MaterialId(0), SectionId(0)));
    }
    m
}
#[test]
fn cantilever_tip_mass_recovers_massless_rotation() {
    let m = independent_cantilevers(1, 0.0);
    let r = analyze_modal(
        &m,
        &ModalOptions {
            modes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let mode = &r.modes[0];
    let expected = 3.0 * 200e9 * 4e-5 / (27.0 * 100.0);
    assert!((mode.eigenvalue / expected - 1.0).abs() < 1e-10);
    assert!((mode.shape[1][5] / mode.shape[1][1] - 0.5).abs() < 1e-10);
    assert!((mode.mass_ratio[1] - 1.0).abs() < 1e-10);
    assert_eq!(modal_inertia_count(&m, mode.frequency_hz * 0.9).unwrap(), 0);
    assert_eq!(modal_inertia_count(&m, mode.frequency_hz * 1.1).unwrap(), 1);
}
#[test]
fn repeated_and_closely_spaced_modes() {
    for split in [0.0, 1e-7] {
        let m = independent_cantilevers(4, split);
        let r = analyze_modal(
            &m,
            &ModalOptions {
                modes: 4,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.modes.len(), 4);
        assert!(r.maximum_mass_orthogonality_error < 1e-8);
        assert!((r.cumulative_mass_ratio[1] - 1.0).abs() < 1e-8);
        assert_eq!(
            modal_inertia_count(&m, r.modes.last().unwrap().frequency_hz * 1.001).unwrap(),
            4
        );
    }
}
#[test]
fn large_operator_uses_partial_krylov_schur() {
    let m = independent_cantilevers(80, 0.1);
    let r = analyze_modal(
        &m,
        &ModalOptions {
            modes: 5,
            ..Default::default()
        },
    )
    .unwrap();
    for (i, mode) in r.modes.iter().enumerate() {
        let expected = 3.0 * 200e9 * 4e-5 / (27.0 * 100.0 * (1.0 + (79 - i) as f64 * 0.1));
        assert!((mode.eigenvalue / expected - 1.0).abs() < 1e-7);
    }
    assert_eq!(r.sturm_count_below_cutoff, Some(4));
}

#[test]
fn large_repeated_modes_pass_the_backend_gate() {
    let m = independent_cantilevers(80, 0.0);
    let r = analyze_modal(
        &m,
        &ModalOptions {
            modes: 5,
            max_restarts: 20,
            ..Default::default()
        },
    )
    .unwrap();
    let expected = 3.0 * 200e9 * 4e-5 / (27.0 * 100.0);
    for mode in r.modes {
        assert!((mode.eigenvalue / expected - 1.0).abs() < 1e-8);
    }
}
#[test]
fn rejects_massless_model_and_free_rigid_body() {
    let mut m = independent_cantilevers(1, 0.0);
    m.nodes[1].mass = [Mass::ZERO; 3];
    assert!(analyze_modal(&m, &Default::default()).is_err());
    m.nodes[1].mass[1] = Mass::from_si(1.0);
    m.nodes[0].restrained = [false; 6];
    assert!(matches!(
        analyze_modal(&m, &Default::default()),
        Err(Error::Unstable(_))
    ));
}
