//! Timoshenko frames: sections with shear areas deform in shear as well as
//! bending. Checked against closed forms and against a finer mesh with the
//! same loads applied at nodes.
use oa_core::{units::*, *};

const E: f64 = 200e9;
const G: f64 = 200e9 / 2.6;
const IY: f64 = 2e-5;
const IZ: f64 = 4e-5;
const AS_Y: f64 = 2e-4;
const AS_Z: f64 = 3e-4;
const L: f64 = 3.0;

fn close(actual: f64, expected: f64, tol: f64) {
    assert!(
        (actual - expected).abs() <= tol * expected.abs().max(1e-12),
        "actual {actual:e}, expected {expected:e}"
    );
}
/// A straight member along global X, fixed at x = 0 and, when `far` is
/// fixed, at x = L, with nodes at the given positions. Local axes are global.
fn beam(stations: &[f64], far_fixed: bool) -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_si(E),
        poisson: 0.3,
        density: MassDensity::ZERO,
    });
    m.add_section(Section {
        area: Area::from_si(0.01),
        iy: SecondMoment::from_si(IY),
        iz: SecondMoment::from_si(IZ),
        torsion: SecondMoment::from_si(1e-5),
        shear_y: Some(Area::from_si(AS_Y)),
        shear_z: Some(Area::from_si(AS_Z)),
    });
    let last = stations.len() - 1;
    for (i, x) in stations.iter().enumerate() {
        let p = [Length::from_si(*x), Length::ZERO, Length::ZERO];
        let fixed = i == 0 || (i == last && far_fixed);
        m.add_node(if fixed { Node::fixed(p) } else { Node::new(p) });
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
fn solve(m: &Model, method: StaticMethod) -> CombinationResult {
    let options = StaticOptions {
        method,
        ..Default::default()
    };
    analyze_static(m, &options).unwrap().combinations.remove(0)
}

#[test]
fn cantilever_tip_adds_shear_to_bending() {
    let (py, pz, mz) = (-2000.0, 3000.0, 700.0);
    let mut m = beam(&[0.0, L], false);
    m.add_load_case(LoadCase {
        name: "tip".into(),
        nodal: vec![NodalLoad {
            node: NodeId(1),
            force: [Force::ZERO, Force::from_si(py), Force::from_si(pz)],
            moment: [Moment::ZERO, Moment::ZERO, Moment::from_si(mz)],
        }],
        ..Default::default()
    });
    let c = solve(&m, StaticMethod::Linear);
    let u = c.displacements.as_ref().unwrap()[1];
    // A tip moment bends without shear; a tip force adds P L / (G As).
    close(
        u[1],
        py * L.powi(3) / (3.0 * E * IZ) + py * L / (G * AS_Y) + mz * L * L / (2.0 * E * IZ),
        1e-10,
    );
    close(
        u[2],
        pz * L.powi(3) / (3.0 * E * IY) + pz * L / (G * AS_Z),
        1e-10,
    );
    // End rotations are section rotations, which shear leaves alone.
    close(u[5], py * L * L / (2.0 * E * IZ) + mz * L / (E * IZ), 1e-10);
    close(u[4], -pz * L * L / (2.0 * E * IY), 1e-10);

    // Along the member the diagram carries the shear strain too.
    let f = &c.frames.as_ref().unwrap()[0];
    let d = frame_diagram(&m, &m.effective_combinations()[0], FrameId(0), f, 5).unwrap();
    for (x, w) in d.stations.iter().zip(&d.deflections) {
        let bending = pz * x * x * (3.0 * L - x) / (6.0 * E * IY);
        close(w[1], bending + pz * x / (G * AS_Z), 1e-9);
    }
    close(d.deflections[4][0], u[1], 1e-9);
}

/// Span loads on one fixed-fixed element give the same end reactions as a
/// mesh with nodes under the loads, since the Timoshenko shape functions
/// are exact; and the diagram of the single element deflects to the finer
/// mesh's nodal displacements.
#[test]
fn span_loads_match_a_mesh_with_nodes_under_them() {
    let (a, b) = (0.75, 1.8);
    let force = [Force::ZERO, Force::from_si(-4000.0), Force::from_si(2500.0)];
    let moment = [
        Moment::ZERO,
        Moment::from_si(1500.0),
        Moment::from_si(-900.0),
    ];
    let q = [
        LineLoad::ZERO,
        LineLoad::from_si(-1200.0),
        LineLoad::from_si(600.0),
    ];
    let uniform = |member: usize, start: f64, end: f64| MemberLoad::Distributed {
        member: FrameId(member),
        start: Length::from_si(start),
        end: Length::from_si(end),
        start_load: q,
        end_load: q,
        axes: Axes::Local,
    };

    let mut coarse = beam(&[0.0, L], true);
    coarse.add_load_case(LoadCase {
        name: "span".into(),
        member: vec![
            MemberLoad::Point {
                member: FrameId(0),
                position: Length::from_si(a),
                force,
                moment: [Moment::ZERO; 3],
                axes: Axes::Local,
            },
            MemberLoad::Point {
                member: FrameId(0),
                position: Length::from_si(b),
                force: [Force::ZERO; 3],
                moment,
                axes: Axes::Local,
            },
            uniform(0, 0.0, L),
        ],
        ..Default::default()
    });
    let mut fine = beam(&[0.0, a, b, L], true);
    fine.add_load_case(LoadCase {
        name: "span".into(),
        nodal: vec![
            NodalLoad {
                node: NodeId(1),
                force,
                moment: [Moment::ZERO; 3],
            },
            NodalLoad {
                node: NodeId(2),
                force: [Force::ZERO; 3],
                moment,
            },
        ],
        member: vec![
            uniform(0, 0.0, a),
            uniform(1, 0.0, b - a),
            uniform(2, 0.0, L - b),
        ],
        ..Default::default()
    });
    let (c, f) = (
        solve(&coarse, StaticMethod::Linear),
        solve(&fine, StaticMethod::Linear),
    );
    let (rc, rf) = (c.reactions.as_ref().unwrap(), f.reactions.as_ref().unwrap());
    for dof in 0..6 {
        close(rc[0][dof], rf[0][dof], 1e-9);
        close(rc[1][dof], rf[3][dof], 1e-9);
    }
    // Stations every 0.15 m land on both load points.
    let frame = &c.frames.as_ref().unwrap()[0];
    let combination = &coarse.effective_combinations()[0];
    let d = frame_diagram(&coarse, combination, FrameId(0), frame, 21).unwrap();
    let u = f.displacements.as_ref().unwrap();
    for (station, node) in [(5, 1), (12, 2)] {
        close(
            d.stations[station],
            fine.nodes[node].position[0].si(),
            1e-12,
        );
        close(d.deflections[station][0], u[node][1], 1e-8);
        close(d.deflections[station][1], u[node][2], 1e-8);
    }
}

/// A cantilever under axial compression P and a tip lateral force H. With
/// shear strain gamma = (H + P v') / (G As), the deflection is
/// [(H / P)(tan kL / k - L) + H L / (G As)] / s, where s = 1 - P / (G As)
/// and k^2 = P / (s E I). This is Engesser's model, and P-Delta must
/// converge to it.
#[test]
fn p_delta_cantilever_matches_engesser() {
    let segments = 16;
    let stations: Vec<f64> = (0..=segments)
        .map(|i| L * i as f64 / segments as f64)
        .collect();
    let (p, h) = (1.0e6, 1000.0);
    let mut m = beam(&stations, false);
    m.add_load_case(LoadCase {
        name: "column".into(),
        nodal: vec![NodalLoad::force(
            NodeId(segments),
            [Force::from_si(-p), Force::from_si(h), Force::ZERO],
        )],
        ..Default::default()
    });
    let c = solve(&m, StaticMethod::PDelta);
    let ga = G * AS_Y;
    let s = 1.0 - p / ga;
    let k = (p / (s * E * IZ)).sqrt();
    let expected = ((h / p) * ((k * L).tan() / k - L) + h * L / ga) / s;
    let tip = c.displacements.as_ref().unwrap()[segments][1];
    close(tip, expected, 1e-4);
    // Without shear the same column is stiffer.
    let rigid = ((h / p) * ((L * (p / (E * IZ)).sqrt()).tan() / (p / (E * IZ)).sqrt() - L)).abs();
    assert!(
        tip > rigid * 1.1,
        "shear should soften the column: {tip:e} vs {rigid:e}"
    );
}

#[test]
fn rejects_nonpositive_shear_area() {
    for bad in [0.0, -1e-4, f64::NAN] {
        let mut m = beam(&[0.0, L], false);
        m.sections[0].shear_z = Some(Area::from_si(bad));
        assert!(m.validate().is_err(), "shear area {bad}");
    }
}

#[test]
fn shear_areas_are_optional_in_json() {
    let mut m = beam(&[0.0, L], false);
    m.sections[0].shear_y = None;
    m.sections[0].shear_z = None;
    let json = serde_json::to_value(&m.sections[0]).unwrap();
    assert!(json.get("shear_y").is_none() && json.get("shear_z").is_none());
    let back: Section = serde_json::from_value(json).unwrap();
    assert!(back.shear_y.is_none());
}
