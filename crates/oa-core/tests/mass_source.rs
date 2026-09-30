//! Mass from load cases: gravity loads in the mass source become lumped
//! translational mass, checked against hand calculations on cantilevers
//! whose fixed end drops half of every lumped member mass from the free total.
use oa_core::{units::*, *};

const E: f64 = 200e9;
const IY: f64 = 2e-5;
const L: f64 = 3.0;
const G: f64 = STANDARD_GRAVITY;

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-9 * expected.abs().max(1e-12),
        "{actual} != {expected}"
    );
}

/// A 3 m cantilever along X, fixed at node 0, with one empty load case.
fn cantilever(density: f64) -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_si(E),
        poisson: 0.3,
        density: MassDensity::from_si(density),
    });
    m.add_section(Section {
        area: Area::from_si(0.01),
        iy: SecondMoment::from_si(IY),
        iz: SecondMoment::from_si(4e-5),
        torsion: SecondMoment::from_si(1e-5),
        shear_y: None,
        shear_z: None,
    });
    let a = m.add_node(Node::fixed([Length::ZERO; 3]));
    let b = m.add_node(Node::new([Length::from_si(L), Length::ZERO, Length::ZERO]));
    m.add_frame(Frame::new([a, b], MaterialId(0), SectionId(0)));
    m.add_load_case(LoadCase {
        name: "SDL".into(),
        ..Default::default()
    });
    m
}

fn modal(m: &Model) -> ModalResult {
    analyze_modal(
        m,
        &ModalOptions {
            modes: 1,
            ..Default::default()
        },
    )
    .unwrap()
}

/// Mass at the free tip, the same in X, Y and Z.
fn tip_mass(m: &Model) -> f64 {
    let r = modal(m);
    close(r.total_free_mass[1], r.total_free_mass[0]);
    close(r.total_free_mass[2], r.total_free_mass[0]);
    r.total_free_mass[0]
}

#[test]
fn loads_are_not_mass_by_default() {
    let mut m = cantilever(7850.0);
    m.load_cases[0].nodal.push(NodalLoad::force(
        NodeId(1),
        [Force::ZERO, Force::ZERO, Force::from_si(-5e3)],
    ));
    close(tip_mass(&m), 7850.0 * 0.01 * L / 2.0);
    // The default mass source is left out of the JSON, so existing content
    // hashes do not change.
    assert!(
        serde_json::to_value(&m)
            .unwrap()
            .get("mass_source")
            .is_none()
    );
}

#[test]
fn nodal_gravity_load_becomes_tip_mass() {
    let mut m = cantilever(0.0);
    let p = 9.80665e3;
    m.load_cases[0].nodal.push(NodalLoad {
        node: NodeId(1),
        // Horizontal forces and moments carry no mass.
        force: [Force::from_si(4e3), Force::ZERO, Force::from_si(-p)],
        moment: [Moment::from_si(2e3); 3],
    });
    m.mass_source.cases = vec![(LoadCaseId(0), 0.25)];
    let mass = 0.25 * p / G;
    close(tip_mass(&m), mass);
    // The lowest mode bends about local y: omega² = 3EI / (L³ m).
    close(
        modal(&m).modes[0].eigenvalue,
        3.0 * E * IY / (L.powi(3) * mass),
    );
}

#[test]
fn member_loads_split_statically_between_the_ends() {
    // A point load a third of the way along puts a third of itself at the tip.
    let mut m = cantilever(0.0);
    m.load_cases[0].member.push(MemberLoad::Point {
        member: FrameId(0),
        position: Length::from_si(1.0),
        force: [Force::ZERO, Force::ZERO, Force::from_si(-3e3)],
        moment: [Moment::ZERO; 3],
        axes: Axes::Global,
    });
    m.mass_source.cases = vec![(LoadCaseId(0), 1.0)];
    close(tip_mass(&m), 3e3 / 3.0 / G);

    // A partial trapezoid carries ∫ q(x) x / L dx to the tip.
    let (a, b, q0, q1) = (0.5, 2.5, 2e3, 6e3);
    let mut m = cantilever(0.0);
    m.load_cases[0].member.push(MemberLoad::Distributed {
        member: FrameId(0),
        start: Length::from_si(a),
        end: Length::from_si(b),
        start_load: [LineLoad::ZERO, LineLoad::ZERO, LineLoad::from_si(-q0)],
        end_load: [LineLoad::ZERO, LineLoad::ZERO, LineLoad::from_si(-q1)],
        axes: Axes::Global,
    });
    m.mass_source.cases = vec![(LoadCaseId(0), 1.0)];
    let slope = (q1 - q0) / (b - a);
    let moment = (q0 - slope * a) * (b * b - a * a) / 2.0 + slope * (b.powi(3) - a.powi(3)) / 3.0;
    close(tip_mass(&m), moment / L / G);
}

#[test]
fn local_member_loads_count_their_global_gravity_component() {
    // Rolled 90°, local y is global Z, so a load along -y is gravity and one
    // along local z, now horizontal, is not.
    let mut m = cantilever(0.0);
    m.frames[0].roll = Angle::from_si(std::f64::consts::FRAC_PI_2);
    let q = 4e3;
    m.load_cases[0].member.push(MemberLoad::Distributed {
        member: FrameId(0),
        start: Length::ZERO,
        end: Length::from_si(L),
        start_load: [
            LineLoad::ZERO,
            LineLoad::from_si(-q),
            LineLoad::from_si(7e3),
        ],
        end_load: [
            LineLoad::ZERO,
            LineLoad::from_si(-q),
            LineLoad::from_si(7e3),
        ],
        axes: Axes::Local,
    });
    m.mass_source.cases = vec![(LoadCaseId(0), 1.0)];
    close(tip_mass(&m), q * L / 2.0 / G);
}

/// A 2 m x 3 m horizontal plate fixed along its y = 0 edge.
fn plate(upward_normal: bool) -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_si(30e9),
        poisson: 0.2,
        density: MassDensity::ZERO,
    });
    let corners = [(0.0, 0.0), (2.0, 0.0), (2.0, 3.0), (0.0, 3.0)];
    let nodes: Vec<NodeId> = corners
        .iter()
        .map(|&(x, y)| {
            let p = [Length::from_si(x), Length::from_si(y), Length::ZERO];
            m.add_node(if y == 0.0 {
                Node::fixed(p)
            } else {
                Node::new(p)
            })
        })
        .collect();
    let mut order = [nodes[0], nodes[1], nodes[2], nodes[3]];
    if !upward_normal {
        order = [nodes[1], nodes[0], nodes[3], nodes[2]];
    }
    m.add_shell(Shell {
        nodes: order,
        material: MaterialId(0),
        thickness: Length::from_si(0.2),
        formulation: ShellFormulation::Dkmq,
        drilling_ratio: 1e-3,
        local_x: None,
        modifiers: Default::default(),
    });
    m.add_load_case(LoadCase {
        name: "SDL".into(),
        ..Default::default()
    });
    m
}

#[test]
fn surface_load_along_gravity_becomes_tributary_mass() {
    let p = 2.4e3;
    for upward in [true, false] {
        let mut m = plate(upward);
        // Pressure acts along the shell normal, so the downward load has the
        // opposite sign on a shell facing down.
        let pressure = if upward { -p } else { p };
        m.load_cases[0].surface.push(SurfaceLoad {
            shell: ShellId(0),
            pressure: Pressure::from_si(pressure),
        });
        m.mass_source.cases = vec![(LoadCaseId(0), 1.0)];
        // Half of the 6 m² plate is tributary to the free corners.
        close(tip_mass(&m), p * 3.0 / G);
    }
}

#[test]
fn self_weight_in_a_source_case_is_element_mass() {
    let density = 7850.0;
    let own = modal(&cantilever(density));
    let mut m = cantilever(density);
    m.load_cases[0].self_weight = [0.0, 0.0, -1.0];
    m.mass_source.cases = vec![(LoadCaseId(0), 1.0)];
    // With element mass on too, self-weight would count twice.
    let error = modal_error(&m);
    assert!(error.contains("self-weight"), "{error}");
    m.mass_source.element_mass = false;
    let r = modal(&m);
    close(r.total_free_mass[0], own.total_free_mass[0]);
    close(r.modes[0].eigenvalue, own.modes[0].eigenvalue);
}

#[test]
fn element_mass_can_be_left_out() {
    let mut m = cantilever(7850.0);
    m.nodes[1].mass = [Mass::from_si(50.0); 3];
    m.mass_source.element_mass = false;
    close(tip_mass(&m), 50.0);
}

fn modal_error(m: &Model) -> String {
    analyze_modal(m, &ModalOptions::default())
        .unwrap_err()
        .to_string()
}

#[test]
fn refuses_invalid_sources_and_negative_mass() {
    let mut m = cantilever(0.0);
    m.mass_source.cases = vec![(LoadCaseId(1), 1.0)];
    assert!(modal_error(&m).contains("missing or listed twice"));
    m.mass_source.cases = vec![(LoadCaseId(0), 1.0), (LoadCaseId(0), 0.5)];
    assert!(modal_error(&m).contains("missing or listed twice"));
    for multiplier in [0.0, -1.0, f64::NAN] {
        m.mass_source.cases = vec![(LoadCaseId(0), multiplier)];
        assert!(modal_error(&m).contains("positive and finite"));
    }
    // An uplift larger than the node's own mass would leave it negative.
    m.nodes[1].mass = [Mass::from_si(10.0); 3];
    m.load_cases[0].nodal.push(NodalLoad::force(
        NodeId(1),
        [Force::ZERO, Force::ZERO, Force::from_si(200.0)],
    ));
    m.mass_source.cases = vec![(LoadCaseId(0), 1.0)];
    assert!(modal_error(&m).contains("negative mass"));
}

#[test]
fn spectrum_base_shear_uses_source_mass() {
    let mut m = cantilever(0.0);
    let p = 9.80665e4;
    m.load_cases[0].nodal.push(NodalLoad::force(
        NodeId(1),
        [Force::ZERO, Force::ZERO, Force::from_si(-p)],
    ));
    m.mass_source.cases = vec![(LoadCaseId(0), 1.0)];
    let flat = |a| SpectrumPoint {
        period_seconds: a,
        acceleration: Acceleration::from_si(2.0),
    };
    let r = analyze_spectrum(
        &m,
        &SpectrumOptions {
            modal: ModalOptions {
                modes: 1,
                ..Default::default()
            },
            // The one mode computed bends in Z, about the weaker axis.
            direction: [0.0, 0.0, 1.0],
            spectrum: vec![flat(0.0), flat(10.0)],
            ..Default::default()
        },
    )
    .unwrap();
    close(r.base_reaction[2], p / G * 2.0);
}

#[test]
fn mass_and_weight_modifiers_scale_own_mass_and_self_weight() {
    let density = 7850.0;
    let own = density * 0.01 * L / 2.0;
    let mut m = cantilever(density);
    m.frames[0].modifiers.mass = 0.5;
    m.frames[0].modifiers.weight = 0.0;
    close(tip_mass(&m), 0.5 * own);
    // Weight is what self-weight applies, so a zero weight loads nothing.
    m.load_cases[0].self_weight = [0.0, 0.0, -1.0];
    let r = analyze_static(&m, &Default::default()).unwrap();
    let reactions = r.combinations[0].reactions.as_ref().unwrap();
    assert_eq!(reactions[0][2], 0.0);
    // A source case's self-weight becomes mass through the weight modifier.
    m.frames[0].modifiers.weight = 0.25;
    m.mass_source.element_mass = false;
    m.mass_source.cases = vec![(LoadCaseId(0), 1.0)];
    close(tip_mass(&m), 0.25 * own);
    m.frames[0].modifiers.mass = -0.5;
    assert!(modal_error(&m).contains("mass and weight modifiers"));
}

#[test]
fn lateral_and_vertical_mass_can_each_be_left_out() {
    let mut m = cantilever(0.0);
    m.nodes[1].mass = [Mass::from_si(40.0); 3];
    // Inertia about X and Y only, so the lateral mode stays a closed form.
    m.nodes[1].mass_inertia = [3.0, 3.0, 0.0].map(MassInertia::from_si);
    m.mass_source.vertical = false;
    close(modal(&m).total_free_mass[0], 40.0);
    assert_eq!(modal(&m).total_free_mass[2], 0.0);
    // Only lateral mass left: the lowest mode is the stiffer lateral one.
    close(
        modal(&m).modes[0].eigenvalue,
        3.0 * E * 4e-5 / (L.powi(3) * 40.0),
    );
    m.mass_source.vertical = true;
    m.mass_source.lateral = false;
    assert_eq!(modal(&m).total_free_mass[0], 0.0);
    close(modal(&m).total_free_mass[2], 40.0);
    m.mass_source.vertical = false;
    assert!(modal_error(&m).contains("lateral or vertical"));
}

/// A 6 m column in two 3 m elements up Z, fixed at node 0, with 50 kg at
/// its middle node in every direction.
fn column() -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_si(E),
        poisson: 0.3,
        density: MassDensity::ZERO,
    });
    m.add_section(Section {
        area: Area::from_si(0.01),
        iy: SecondMoment::from_si(IY),
        iz: SecondMoment::from_si(IY),
        torsion: SecondMoment::from_si(1e-5),
        shear_y: None,
        shear_z: None,
    });
    let z = |z: f64| [Length::ZERO, Length::ZERO, Length::from_si(z)];
    let a = m.add_node(Node::fixed(z(0.0)));
    let mut middle = Node::new(z(3.0));
    middle.mass = [Mass::from_si(50.0); 3];
    let b = m.add_node(middle);
    let c = m.add_node(Node::new(z(6.0)));
    m.add_frame(Frame::new([a, b], MaterialId(0), SectionId(0)));
    m.add_frame(Frame::new([b, c], MaterialId(0), SectionId(0)));
    m
}

#[test]
fn lumping_moves_lateral_mass_only() {
    let mut lumped = column();
    lumped.mass_source.lump = vec![(NodeId(1), vec![(NodeId(2), 1.0)])];
    let mut moved = column();
    moved.nodes[1].mass = [Mass::ZERO, Mass::ZERO, Mass::from_si(50.0)];
    moved.nodes[2].mass = [Mass::from_si(50.0), Mass::from_si(50.0), Mass::ZERO];
    let (a, b) = (modal(&lumped), modal(&moved));
    close(a.modes[0].eigenvalue, b.modes[0].eigenvalue);
    close(a.total_free_mass[2], 50.0);
    // Half onto the fixed base leaves half of it free.
    lumped.mass_source.lump = vec![(NodeId(1), vec![(NodeId(0), 0.5), (NodeId(2), 0.5)])];
    close(modal(&lumped).total_free_mass[0], 25.0);
    for bad in [
        vec![(NodeId(1), vec![(NodeId(2), 0.6)])],
        vec![(NodeId(1), vec![(NodeId(7), 1.0)])],
        vec![
            (NodeId(1), vec![(NodeId(2), 1.0)]),
            (NodeId(2), vec![(NodeId(0), 1.0)]),
        ],
    ] {
        lumped.mass_source.lump = bad;
        assert!(modal_error(&lumped).contains("lump"));
    }
}
