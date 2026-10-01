//! Rigid end offsets and joint offsets: the member runs between its moved
//! ends, its rigid zones carry loads straight to the nodes, and its section
//! forces are reported over the clear length. Checked against closed forms
//! and against the same member with its rigid zones modelled as very stiff
//! members.
use oa_core::{units::*, *};

const E: f64 = 200e9;
const A: f64 = 0.01;
const IY: f64 = 2e-5;
const IZ: f64 = 4e-5;
const J: f64 = 1e-5;
const L: f64 = 3.0;

fn close(actual: f64, expected: f64, tol: f64) {
    assert!(
        (actual - expected).abs() <= tol * expected.abs().max(1e-12),
        "actual {actual:e}, expected {expected:e}"
    );
}
/// Each entry within `tol` of the largest in `expected`.
fn close_all(actual: &[f64], expected: &[f64], tol: f64) {
    let scale = expected.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
    for (k, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (a - e).abs() <= tol * scale,
            "entry {k}: actual {a:e}, expected {e:e}"
        );
    }
}
fn len(v: f64) -> Length {
    Length::from_si(v)
}
fn at(x: f64, z: f64) -> [Length; 3] {
    [len(x), Length::ZERO, len(z)]
}
/// One material and two sections: the member's, and the same scaled by
/// `stiff` for rigid zones modelled as members.
fn base(stiff: f64) -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_si(E),
        poisson: 0.3,
        density: MassDensity::from_si(7850.0),
    });
    for s in [1.0, stiff] {
        m.add_section(Section {
            area: Area::from_si(A * s),
            iy: SecondMoment::from_si(IY * s),
            iz: SecondMoment::from_si(IZ * s),
            torsion: SecondMoment::from_si(J * s),
            shear_y: None,
            shear_z: None,
        });
    }
    m
}
fn frame(i: usize, j: usize, section: usize) -> Frame {
    Frame::new([NodeId(i), NodeId(j)], MaterialId(0), SectionId(section))
}
fn solve(m: &Model) -> CombinationResult {
    analyze_static(m, &Default::default())
        .unwrap()
        .combinations
        .remove(0)
}
fn tip_load(node: usize, force: [f64; 3]) -> LoadCase {
    LoadCase {
        name: "load".into(),
        nodal: vec![NodalLoad::force(NodeId(node), force.map(Force::from_si))],
        ..Default::default()
    }
}

#[test]
fn rigid_zone_shortens_a_cantilever() {
    let p = -1000.0;
    for (rigid, offset) in [(1.0, 0.5), (0.5, 0.5), (0.0, 0.5), (1.0, 0.0)] {
        let mut m = base(1.0);
        m.add_node(Node::fixed(at(0.0, 0.0)));
        m.add_node(Node::new(at(L, 0.0)));
        let mut f = frame(0, 1, 0);
        f.offsets.end = [len(offset), Length::ZERO];
        f.offsets.rigid_zone = rigid;
        m.add_frame(f);
        m.add_load_case(tip_load(1, [0.0, p, 0.0]));
        let c = solve(&m);
        let flexible = L - rigid * offset;
        let u = c.displacements.as_ref().unwrap()[1];
        close(u[1], p * flexible.powi(3) / (3.0 * E * IZ), 1e-10);
        close(u[5], p * flexible.powi(2) / (2.0 * E * IZ), 1e-10);
        // Forces are reported at the ends of the flexible part.
        let fr = &c.frames.as_ref().unwrap()[0];
        close(fr.local_end_forces[1], -p, 1e-10);
        close(fr.local_end_forces[5], -p * flexible, 1e-10);
        // The reaction still balances the load about the node.
        let r = c.reactions.as_ref().unwrap()[0];
        close(r[1], -p, 1e-10);
        close(r[5], -p * L, 1e-10);
        // Stations cover the clear length, where the moment is p (L - x).
        let d = frame_diagram(&m, &m.effective_combinations()[0], FrameId(0), fr, 5).unwrap();
        close(d.stations[0], offset, 1e-12);
        close(d.stations[4], L, 1e-12);
        for (x, s) in d.stations.iter().zip(&d.forces) {
            assert!((s[5] - p * (L - x)).abs() < 1e-9 * (p * L).abs());
        }
        close(d.deflections[4][0], u[1], 1e-9);
    }
}

/// A beam on rigid zones, against the same beam with each rigid zone a
/// member 1e6 times stiffer, under a load that runs into both zones and a
/// point load on one.
#[test]
fn rigid_zones_match_very_stiff_members() {
    let (oi, oj) = (0.4, 0.25);
    let loads = |member: usize, start: f64, end: f64| MemberLoad::Distributed {
        member: FrameId(member),
        start: len(start),
        end: len(end),
        start_load: [0.0, -2000.0, -1500.0].map(LineLoad::from_si),
        end_load: [300.0, -4000.0, 500.0].map(LineLoad::from_si),
        axes: Axes::Global,
    };
    let mut offset = base(1e6);
    offset.add_node(Node::fixed(at(0.0, 0.0)));
    offset.add_node(Node::new(at(L, 0.0)));
    let mut f = frame(0, 1, 0);
    f.offsets.end = [len(oi), len(oj)];
    f.offsets.rigid_zone = 1.0;
    offset.add_frame(f);
    offset.add_load_case(LoadCase {
        name: "load".into(),
        member: vec![
            loads(0, 0.0, L),
            MemberLoad::Point {
                member: FrameId(0),
                position: len(0.1),
                force: [0.0, 0.0, -800.0].map(Force::from_si),
                moment: [Moment::ZERO; 3],
                axes: Axes::Global,
            },
        ],
        nodal: vec![NodalLoad::force(
            NodeId(1),
            [100.0, 300.0, -200.0].map(Force::from_si),
        )],
        ..Default::default()
    });

    // The trapezoid split at the zone ends: its value at a cut is linear.
    let q = |x: f64| {
        [
            0.0 + 300.0 * x / L,
            -2000.0 - 2000.0 * x / L,
            -1500.0 + 2000.0 * x / L,
        ]
    };
    let piece = |member: usize, a: f64, b: f64, shift: f64| MemberLoad::Distributed {
        member: FrameId(member),
        start: len(a - shift),
        end: len(b - shift),
        start_load: q(a).map(LineLoad::from_si),
        end_load: q(b).map(LineLoad::from_si),
        axes: Axes::Global,
    };
    let mut split = base(1e6);
    split.add_node(Node::fixed(at(0.0, 0.0)));
    split.add_node(Node::new(at(oi, 0.0)));
    split.add_node(Node::new(at(L - oj, 0.0)));
    split.add_node(Node::new(at(L, 0.0)));
    split.add_frame(frame(0, 1, 1));
    split.add_frame(frame(1, 2, 0));
    split.add_frame(frame(2, 3, 1));
    split.add_load_case(LoadCase {
        name: "load".into(),
        member: vec![
            piece(0, 0.0, oi, 0.0),
            piece(1, oi, L - oj, oi),
            piece(2, L - oj, L, L - oj),
            MemberLoad::Point {
                member: FrameId(0),
                position: len(0.1),
                force: [0.0, 0.0, -800.0].map(Force::from_si),
                moment: [Moment::ZERO; 3],
                axes: Axes::Global,
            },
        ],
        nodal: vec![NodalLoad::force(
            NodeId(3),
            [100.0, 300.0, -200.0].map(Force::from_si),
        )],
        ..Default::default()
    });

    let (a, b) = (solve(&offset), solve(&split));
    let (ua, ub) = (
        a.displacements.as_ref().unwrap()[1],
        b.displacements.as_ref().unwrap()[3],
    );
    close_all(&ua, &ub, 1e-6);
    let (ra, rb) = (
        a.reactions.as_ref().unwrap()[0],
        b.reactions.as_ref().unwrap()[0],
    );
    close_all(&ra, &rb, 1e-6);
    // The flexible part's end forces are the middle member's.
    let (fa, fb) = (
        &a.frames.as_ref().unwrap()[0],
        &b.frames.as_ref().unwrap()[1],
    );
    close_all(&fa.local_end_forces, &fb.local_end_forces, 1e-6);
    // And so are its section forces, at the same point in space.
    let combo = &offset.effective_combinations()[0];
    for x in [oi, 1.0, 2.2, L - oj] {
        let sa = frame_section_forces(&offset, combo, FrameId(0), fa, len(x)).unwrap();
        let sb = frame_section_forces(&split, combo, FrameId(1), fb, len(x - oi)).unwrap();
        close_all(&sa.values, &sb.values, 1e-6);
    }
}

/// A load on a rigid zone reaches its node without passing a release at
/// the face, so a beam pinned at its faces carries none of it.
#[test]
fn rigid_zone_loads_bypass_end_releases() {
    let (o, q) = (0.3, -5000.0);
    let mut m = base(1.0);
    m.add_node(Node::fixed(at(0.0, 0.0)));
    m.add_node(Node::fixed(at(L, 0.0)));
    let mut f = frame(0, 1, 0);
    f.offsets.end = [len(o), len(o)];
    f.offsets.rigid_zone = 1.0;
    // Pinned at both faces in the x-y plane.
    f.releases[5] = true;
    f.releases[11] = true;
    m.add_frame(f);
    m.add_load_case(LoadCase {
        name: "load".into(),
        member: vec![
            MemberLoad::Distributed {
                member: FrameId(0),
                start: Length::ZERO,
                end: len(L),
                start_load: [0.0, q, 0.0].map(LineLoad::from_si),
                end_load: [0.0, q, 0.0].map(LineLoad::from_si),
                axes: Axes::Local,
            },
            MemberLoad::Point {
                member: FrameId(0),
                position: len(0.1),
                force: [0.0, 7000.0, 0.0].map(Force::from_si),
                moment: [Moment::ZERO; 3],
                axes: Axes::Local,
            },
        ],
        ..Default::default()
    });
    let c = solve(&m);
    let fr = &c.frames.as_ref().unwrap()[0];
    let span = L - 2.0 * o;
    // A simply supported span carrying only its own share of the load.
    close(fr.local_end_forces[1], -q * span / 2.0, 1e-10);
    close(fr.local_end_forces[7], -q * span / 2.0, 1e-10);
    let combo = &m.effective_combinations()[0];
    let mid = frame_section_forces(&m, combo, FrameId(0), fr, len(L / 2.0)).unwrap();
    close(mid.values[5], -q * span * span / 8.0, 1e-10);
    // The supports take everything.
    let r = c.reactions.as_ref().unwrap();
    close(r[0][1] + r[1][1], -(q * L + 7000.0), 1e-10);
}

/// A joint offset moves the member off its nodes: an axial load at the
/// nodes bends the member through the links but leaves no moment at the
/// support, since the load's line passes through it.
#[test]
fn joint_offsets_move_the_member_off_its_nodes() {
    let (dz, p) = (-0.2, 4000.0);
    let model = |axes: Axes| {
        let mut m = base(1.0);
        m.add_node(Node::fixed(at(0.0, 0.0)));
        m.add_node(Node::new(at(L, 0.0)));
        let mut f = frame(0, 1, 0);
        f.offsets.joint = [[Length::ZERO, Length::ZERO, len(dz)]; 2];
        f.offsets.axes = axes;
        m.add_frame(f);
        m.add_load_case(tip_load(1, [p, 0.0, 0.0]));
        m
    };
    let m = model(Axes::Global);
    let c = solve(&m);
    let fr = &c.frames.as_ref().unwrap()[0];
    let d = frame_diagram(&m, &m.effective_combinations()[0], FrameId(0), fr, 3).unwrap();
    close(d.origin[2], dz, 1e-12);
    close(d.length, L, 1e-12);
    for s in &d.forces {
        close(s[0], p, 1e-10);
        // The load sits above the member, so it hogs: My = -p dz in the
        // sign convention of the cut, the same all along.
        close(s[4].abs(), (p * dz).abs(), 1e-10);
        close(s[4], d.forces[0][4], 1e-12);
    }
    let r = c.reactions.as_ref().unwrap()[0];
    close(r[0], -p, 1e-10);
    for (k, v) in r.iter().enumerate().skip(1) {
        assert!(v.abs() < 1e-6, "reaction {k} is {v}");
    }
    // The node moves with the member end and its rotation: the member
    // stretches p L / EA and its end turns p dz L / EIy, which swings the
    // node on its link of length dz a further p dz² L / EIy along x.
    let u = c.displacements.as_ref().unwrap()[1];
    close(u[4].abs(), (p * dz * L / (E * IY)).abs(), 1e-10);
    close(u[0], p * L / (E * A) + p * dz * dz * L / (E * IY), 1e-9);

    // For a member along global X, local z is global Z, so the same offset
    // in local axes gives the same answer.
    let local = solve(&model(Axes::Local));
    let ul = local.displacements.as_ref().unwrap()[1];
    for k in 0..6 {
        close(ul[k], u[k], 1e-12);
    }
}

#[test]
fn offsets_are_validated() {
    let check = |edit: &dyn Fn(&mut Frame)| {
        let mut m = base(1.0);
        m.add_node(Node::fixed(at(0.0, 0.0)));
        m.add_node(Node::new(at(L, 0.0)));
        let mut f = frame(0, 1, 0);
        edit(&mut f);
        m.add_frame(f);
        m.add_load_case(tip_load(1, [0.0, 1.0, 0.0]));
        // The clear length is an element check, as orientation is.
        m.validate().and_then(|_| m.validate_frame(0))
    };
    assert!(check(&|_| {}).is_ok());
    assert!(check(&|f| f.offsets.end = [len(1.5), len(1.5)]).is_err());
    assert!(check(&|f| f.offsets.end = [len(-0.1), Length::ZERO]).is_err());
    assert!(check(&|f| f.offsets.rigid_zone = 1.5).is_err());
    assert!(check(&|f| f.offsets.joint[1][0] = len(-L)).is_err());
    assert!(check(&|f| f.offsets.joint[0][2] = len(f64::NAN)).is_err());
    // Member loads run along the member between its moved ends.
    let mut m = base(1.0);
    m.add_node(Node::fixed(at(0.0, 0.0)));
    m.add_node(Node::new(at(L, 0.0)));
    let mut f = frame(0, 1, 0);
    f.offsets.joint[1][0] = len(1.0);
    m.add_frame(f);
    m.add_load_case(LoadCase {
        name: "load".into(),
        member: vec![MemberLoad::Point {
            member: FrameId(0),
            position: len(L + 0.5),
            force: [0.0, -1.0, 0.0].map(Force::from_si),
            moment: [Moment::ZERO; 3],
            axes: Axes::Global,
        }],
        ..Default::default()
    });
    assert!(m.validate().is_ok());
    assert!(analyze_static(&m, &Default::default()).is_ok());
}

/// Offsets that are all zero leave the JSON, and so the content hash, as
/// it was.
#[test]
fn unset_offsets_stay_out_of_the_json() {
    let f = frame(0, 1, 0);
    let json = serde_json::to_value(&f).unwrap();
    assert!(json.get("offsets").is_none());
    let mut g = f.clone();
    g.offsets.rigid_zone = 0.5;
    let json = serde_json::to_value(&g).unwrap();
    assert_eq!(json["offsets"]["rigid_zone"], 0.5);
    let back: Frame = serde_json::from_value(json).unwrap();
    assert_eq!(back.offsets, g.offsets);
}

/// Mass and self-weight count the whole member, joints included.
#[test]
fn mass_counts_the_whole_member() {
    let mut m = base(1.0);
    m.add_node(Node::fixed(at(0.0, 0.0)));
    m.add_node(Node::fixed(at(L, 0.0)));
    let mut f = frame(0, 1, 0);
    f.offsets.end = [len(0.5), len(0.5)];
    f.offsets.rigid_zone = 1.0;
    m.add_frame(f);
    m.add_load_case(LoadCase {
        name: "dead".into(),
        self_weight: [0.0, 0.0, -1.0],
        ..Default::default()
    });
    let c = solve(&m);
    let r = c.reactions.as_ref().unwrap();
    close(r[0][2] + r[1][2], 7850.0 * A * L * STANDARD_GRAVITY, 1e-10);
}
