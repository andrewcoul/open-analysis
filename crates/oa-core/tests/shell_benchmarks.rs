use oa_core::{units::*, *};
fn plate(n: usize, formulation: ShellFormulation) -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_gpa(200.0),
        poisson: 0.3,
        density: MassDensity::ZERO,
    });
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
    let mut case = LoadCase {
        name: "pressure".into(),
        ..Default::default()
    };
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            let id = m.add_shell(Shell {
                nodes: [
                    NodeId(a),
                    NodeId(a + 1),
                    NodeId(a + n + 2),
                    NodeId(a + n + 1),
                ],
                material: MaterialId(0),
                thickness: Length::from_si(0.01),
                formulation,
                drilling_ratio: 1e-3,
            });
            case.surface.push(SurfaceLoad {
                shell: id,
                pressure: Pressure::from_si(-1000.0),
            });
        }
    }
    m.add_load_case(case);
    m
}
#[test]
fn simply_supported_square_thin_plate_convergence() {
    let d = 200e9 * 0.01_f64.powi(3) / (12.0 * (1.0 - 0.3 * 0.3));
    let expected = -0.00406235 * 1000.0 / d;
    for formulation in [ShellFormulation::Rectangular, ShellFormulation::Dkmq] {
        let mut errors = vec![];
        for n in [4, 8] {
            let m = plate(n, formulation);
            let r = analyze_static(&m, &Default::default()).unwrap();
            let center = n / 2 * (n + 1) + n / 2;
            let u = r.combinations[0].displacements.as_ref().unwrap()[center][2];
            errors.push((u / expected - 1.0).abs());
            let sum: f64 = r.combinations[0]
                .reactions
                .as_ref()
                .unwrap()
                .iter()
                .map(|v| v[2])
                .sum();
            assert!(
                (sum - 1000.0).abs() < 1e-5,
                "{formulation:?} vertical equilibrium: {sum}"
            );
        }
        assert!(errors[1] < 0.025, "{formulation:?} errors: {errors:?}");
        assert!(
            errors[1] < errors[0],
            "{formulation:?} fails refinement: {errors:?}"
        );
    }
}
#[test]
fn pynite_logan_rectangular_plate_example_12_1() {
    let mut m = plate(2, ShellFormulation::Rectangular);
    m.materials[0].young = Pressure::from_psi(30e6);
    for n in &mut m.nodes {
        for i in 0..2 {
            n.position[i] = Length::from_inches(n.position[i].si() * 20.0);
        }
        n.restrained = [true; 6];
    }
    m.nodes[4].restrained = [true, true, false, false, false, true];
    for s in &mut m.shells {
        s.thickness = Length::from_inches(0.1);
    }
    m.load_cases[0].surface.clear();
    m.load_cases[0].nodal.push(NodalLoad::force(
        NodeId(4),
        [Force::ZERO, Force::ZERO, Force::from_lbf(-100.0)],
    ));
    let r = analyze_static(&m, &Default::default()).unwrap();
    let actual = r.combinations[0].displacements.as_ref().unwrap()[4][2];
    let expected = -0.0861742424242424 * 0.0254;
    assert!(
        (actual / expected - 1.0).abs() < 1e-10,
        "{actual}, {expected}"
    );
}
#[test]
fn membrane_patch_and_bending_rigid_body_motion() {
    for formulation in [ShellFormulation::Rectangular, ShellFormulation::Dkmq] {
        let mut m = plate(1, formulation);
        m.load_cases[0].surface.clear();
        for node in &mut m.nodes {
            let [x, y, _] = node.xyz();
            node.restrained = [true; 6];
            node.prescribed.translation = [
                Length::from_si(0.001 * x),
                Length::from_si(-0.0003 * y),
                Length::from_si(0.01 * y - 0.02 * x),
            ];
            node.prescribed.rotation = [Angle::from_si(0.01), Angle::from_si(0.02), Angle::ZERO];
        }
        let r = analyze_static(&m, &Default::default()).unwrap();
        let shell = &r.combinations[0].shells.as_ref().unwrap()[0];
        assert!((shell.membrane_stress[0] / 200e6 - 1.0).abs() < 1e-10);
        assert!(shell.membrane_stress[1].abs() < 1e-6);
        assert!(
            shell.bending_moment.iter().all(|v| v.abs() < 1e-7),
            "{formulation:?} {:?}",
            shell.bending_moment
        );
        assert!(shell.transverse_shear.iter().all(|v| v.abs() < 1e-7));
    }
}
#[test]
fn rejects_warped_and_crossed_quads() {
    let mut m = plate(1, ShellFormulation::Dkmq);
    m.nodes[3].position[2] = Length::from_si(0.1);
    assert!(analyze_static(&m, &Default::default()).is_err());
    m.nodes[3].position[2] = Length::ZERO;
    m.shells[0].nodes.swap(1, 2);
    assert!(analyze_static(&m, &Default::default()).is_err());
}
