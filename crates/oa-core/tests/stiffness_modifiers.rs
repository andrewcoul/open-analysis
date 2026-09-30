//! Stiffness modifiers: each scales one stiffness term of a frame or shell
//! and leaves mass and self-weight alone. Checked against closed forms and
//! against the unmodified element.
use oa_core::{units::*, *};

const E: f64 = 200e9;
const G: f64 = 200e9 / 2.6;
const A: f64 = 0.01;
const IY: f64 = 2e-5;
const IZ: f64 = 4e-5;
const J: f64 = 1e-5;
const AS_Y: f64 = 2e-4;
const AS_Z: f64 = 3e-4;
const L: f64 = 3.0;

fn close(actual: f64, expected: f64, tol: f64) {
    assert!(
        (actual - expected).abs() <= tol * expected.abs().max(1e-12),
        "actual {actual:e}, expected {expected:e}"
    );
}
/// A cantilever along global X, fixed at x = 0, in `elements` equal parts.
fn cantilever(elements: usize, density: f64, shear: bool, modifiers: FrameModifiers) -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_si(E),
        poisson: 0.3,
        density: MassDensity::from_si(density),
    });
    m.add_section(Section {
        area: Area::from_si(A),
        iy: SecondMoment::from_si(IY),
        iz: SecondMoment::from_si(IZ),
        torsion: SecondMoment::from_si(J),
        shear_y: shear.then(|| Area::from_si(AS_Y)),
        shear_z: shear.then(|| Area::from_si(AS_Z)),
    });
    for i in 0..=elements {
        let p = [
            Length::from_si(L * i as f64 / elements as f64),
            Length::ZERO,
            Length::ZERO,
        ];
        m.add_node(if i == 0 { Node::fixed(p) } else { Node::new(p) });
        if i > 0 {
            let mut frame = Frame::new([NodeId(i - 1), NodeId(i)], MaterialId(0), SectionId(0));
            frame.modifiers = modifiers;
            m.add_frame(frame);
        }
    }
    m
}
fn uniform(f: f64) -> FrameModifiers {
    FrameModifiers {
        area: f,
        shear_y: f,
        shear_z: f,
        torsion: f,
        iy: f,
        iz: f,
    }
}

#[test]
fn frame_modifiers_scale_each_stiffness_term() {
    let md = FrameModifiers {
        area: 0.5,
        shear_y: 0.4,
        shear_z: 0.6,
        torsion: 0.3,
        iy: 0.7,
        iz: 0.35,
    };
    let (px, py, pz, mx) = (5000.0, -2000.0, 3000.0, 400.0);
    let mut m = cantilever(1, 0.0, true, md);
    m.add_load_case(LoadCase {
        name: "tip".into(),
        nodal: vec![NodalLoad {
            node: NodeId(1),
            force: [px, py, pz].map(Force::from_si),
            moment: [Moment::from_si(mx), Moment::ZERO, Moment::ZERO],
        }],
        ..Default::default()
    });
    let c = analyze_static(&m, &Default::default())
        .unwrap()
        .combinations
        .remove(0);
    let u = c.displacements.as_ref().unwrap()[1];
    close(u[0], px * L / (md.area * E * A), 1e-10);
    close(
        u[1],
        py * L.powi(3) / (3.0 * md.iz * E * IZ) + py * L / (md.shear_y * G * AS_Y),
        1e-10,
    );
    close(
        u[2],
        pz * L.powi(3) / (3.0 * md.iy * E * IY) + pz * L / (md.shear_z * G * AS_Z),
        1e-10,
    );
    close(u[3], mx * L / (md.torsion * G * J), 1e-10);

    // The diagram integrates the same modified rigidities.
    let f = &c.frames.as_ref().unwrap()[0];
    let d = frame_diagram(&m, &m.effective_combinations()[0], FrameId(0), f, 5).unwrap();
    for (x, w) in d.stations.iter().zip(&d.deflections) {
        let bending = pz * x * x * (3.0 * L - x) / (6.0 * md.iy * E * IY);
        close(w[1], bending + pz * x / (md.shear_z * G * AS_Z), 1e-9);
    }
    close(d.deflections[4][0], u[1], 1e-9);
}

#[test]
fn frame_modifiers_leave_mass_and_self_weight_alone() {
    // Scaling every stiffness term by f with the mass unchanged scales every
    // eigenvalue by f.
    let f = 0.35;
    let options = ModalOptions {
        modes: 4,
        ..Default::default()
    };
    let base = analyze_modal(&cantilever(6, 7850.0, false, uniform(1.0)), &options).unwrap();
    let cracked = analyze_modal(&cantilever(6, 7850.0, false, uniform(f)), &options).unwrap();
    for (a, b) in base.modes.iter().zip(&cracked.modes) {
        close(b.eigenvalue, f * a.eigenvalue, 1e-8);
    }
    close(cracked.total_free_mass[0], base.total_free_mass[0], 1e-12);

    let mut m = cantilever(2, 7850.0, true, uniform(f));
    m.add_load_case(LoadCase {
        name: "self".into(),
        self_weight: [0.0, 0.0, -1.0],
        ..Default::default()
    });
    let r = analyze_static(&m, &Default::default()).unwrap();
    let reaction = r.combinations[0].reactions.as_ref().unwrap()[0][2];
    close(reaction, 7850.0 * A * L * STANDARD_GRAVITY, 1e-10);
}

#[test]
fn modifiers_must_be_positive_and_finite() {
    for bad in [0.0, -0.5, f64::NAN, f64::INFINITY] {
        let mut m = cantilever(
            1,
            0.0,
            false,
            FrameModifiers {
                iz: bad,
                ..Default::default()
            },
        );
        let err = m.validate().unwrap_err().to_string();
        assert!(err.contains("stiffness modifiers"), "{err}");
        m.frames[0].modifiers = Default::default();
        m.add_node(Node::fixed([
            Length::ZERO,
            Length::from_si(1.0),
            Length::ZERO,
        ]));
        m.add_node(Node::fixed([
            Length::from_si(1.0),
            Length::from_si(1.0),
            Length::ZERO,
        ]));
        m.add_shell(Shell {
            nodes: [NodeId(0), NodeId(1), NodeId(3), NodeId(2)],
            material: MaterialId(0),
            thickness: Length::from_si(0.1),
            formulation: ShellFormulation::Dkmq,
            drilling_ratio: 1e-3,
            modifiers: ShellModifiers {
                membrane_shear: bad,
                ..Default::default()
            },
        });
        let err = m.validate().unwrap_err().to_string();
        assert!(err.contains("shell 0: stiffness modifiers"), "{err}");
    }
    // Unmodified members serialize as they did before modifiers existed.
    let json = serde_json::to_string(&cantilever(1, 0.0, false, uniform(1.0))).unwrap();
    assert!(!json.contains("modifiers"));
}

/// A `nx` by `nz` mesh of rectangular shells in the global X-Z plane, fixed
/// along z = 0, with in-plane and out-of-plane forces at the top corners.
fn wall(formulation: ShellFormulation, modifiers: ShellModifiers) -> Model {
    let (nx, nz, width, height) = (2, 4, 2.0, 4.0);
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_gpa(30.0),
        poisson: 0.2,
        density: MassDensity::from_si(2400.0),
    });
    for j in 0..=nz {
        for i in 0..=nx {
            let p = [
                Length::from_si(width * i as f64 / nx as f64),
                Length::ZERO,
                Length::from_si(height * j as f64 / nz as f64),
            ];
            m.add_node(if j == 0 { Node::fixed(p) } else { Node::new(p) });
        }
    }
    for j in 0..nz {
        for i in 0..nx {
            let a = j * (nx + 1) + i;
            m.add_shell(Shell {
                nodes: [a, a + 1, a + nx + 2, a + nx + 1].map(NodeId),
                material: MaterialId(0),
                thickness: Length::from_si(0.2),
                formulation,
                drilling_ratio: 1e-3,
                modifiers,
            });
        }
    }
    let top = nz * (nx + 1);
    m.add_load_case(LoadCase {
        name: "top".into(),
        nodal: [top, top + nx]
            .map(|n| {
                NodalLoad::force(
                    NodeId(n),
                    [Force::from_si(1e5), Force::from_si(2e3), Force::ZERO],
                )
            })
            .to_vec(),
        ..Default::default()
    });
    m
}
fn top_corner(m: &Model) -> [f64; 6] {
    let r = analyze_static(m, &Default::default()).unwrap();
    r.combinations[0].displacements.as_ref().unwrap()[m.nodes.len() - 1]
}

#[test]
fn shell_modifiers_scale_membrane_and_bending() {
    // Rectangular shells have no transverse shear, so one factor on all
    // three terms scales the whole stiffness, drilling included.
    let base = top_corner(&wall(ShellFormulation::Rectangular, Default::default()));
    let f = 0.35;
    let cracked = top_corner(&wall(
        ShellFormulation::Rectangular,
        ShellModifiers {
            membrane: f,
            membrane_shear: f,
            bending: f,
        },
    ));
    for i in 0..3 {
        close(cracked[i], base[i] / f, 1e-8);
    }
    // Membrane factors leave out-of-plane bending alone, and the bending
    // factor leaves the in-plane response alone.
    let membrane = top_corner(&wall(
        ShellFormulation::Rectangular,
        ShellModifiers {
            membrane: 0.7,
            membrane_shear: 0.4,
            bending: 1.0,
        },
    ));
    close(membrane[1], base[1], 1e-8);
    assert!(membrane[0] > base[0] / 0.7 && membrane[0] < base[0] / 0.4);
    let bending = top_corner(&wall(
        ShellFormulation::Rectangular,
        ShellModifiers {
            bending: 0.25,
            ..Default::default()
        },
    ));
    close(bending[0], base[0], 1e-8);
    close(bending[1], base[1] / 0.25, 1e-8);

    // DKMQ keeps its transverse shear rigidity, so the out-of-plane
    // deflection grows by a little less than 1 / factor.
    let base = top_corner(&wall(ShellFormulation::Dkmq, Default::default()));
    let bending = top_corner(&wall(
        ShellFormulation::Dkmq,
        ShellModifiers {
            bending: 0.25,
            ..Default::default()
        },
    ));
    close(bending[0], base[0], 1e-8);
    let ratio = bending[1] / base[1];
    assert!(ratio > 3.8 && ratio < 4.0 + 1e-9, "ratio {ratio}");
}

#[test]
fn membrane_modifier_scales_uniaxial_stretch_without_shear() {
    // One element in uniform tension strains only along x, so the in-plane
    // shear factor plays no part.
    let (a, b, t, sigma) = (2.0, 1.0, 0.2, 1e6);
    let stretch = |modifiers: ShellModifiers| {
        let mut m = Model::default();
        m.add_material(Material {
            young: Pressure::from_gpa(30.0),
            poisson: 0.2,
            density: MassDensity::ZERO,
        });
        for (x, y) in [(0.0, 0.0), (a, 0.0), (a, b), (0.0, b)] {
            let mut n = Node::new([Length::from_si(x), Length::from_si(y), Length::ZERO]);
            n.restrained = [x == 0.0, x == 0.0 && y == 0.0, true, true, true, true];
            m.add_node(n);
        }
        m.add_shell(Shell {
            nodes: [0, 1, 2, 3].map(NodeId),
            material: MaterialId(0),
            thickness: Length::from_si(t),
            formulation: ShellFormulation::Dkmq,
            drilling_ratio: 1e-3,
            modifiers,
        });
        m.add_load_case(LoadCase {
            name: "pull".into(),
            nodal: [1, 2]
                .map(|n| {
                    NodalLoad::force(
                        NodeId(n),
                        [
                            Force::from_si(sigma * t * b / 2.0),
                            Force::ZERO,
                            Force::ZERO,
                        ],
                    )
                })
                .to_vec(),
            ..Default::default()
        });
        let r = analyze_static(&m, &Default::default()).unwrap();
        let c = &r.combinations[0];
        (
            c.displacements.as_ref().unwrap()[2][0],
            c.shells.as_ref().unwrap()[0].membrane_stress[0],
        )
    };
    let modifiers = ShellModifiers {
        membrane: 0.7,
        membrane_shear: 0.1,
        bending: 1.0,
    };
    let (u, stress) = stretch(modifiers);
    close(u, sigma * a / (0.7 * 30e9), 1e-10);
    close(stress, sigma, 1e-10);
}
