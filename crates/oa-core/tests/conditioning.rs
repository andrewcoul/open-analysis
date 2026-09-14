//! Badly conditioned systems must still reach the equilibrium tolerance
//! through iterative refinement, and the answer must be right, not merely
//! self-consistent.
use oa_core::{units::*, *};

/// Twenty collinear segments alternating between a stiff section and one
/// `ratio` times softer. Condition number grows with the ratio.
fn contrast_chain(ratio: f64) -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_gpa(200.0),
        poisson: 0.3,
        density: MassDensity::ZERO,
    });
    for divisor in [1.0, ratio] {
        m.add_section(Section {
            area: Area::from_si(0.01 / divisor),
            iy: SecondMoment::from_si(2e-5 / divisor),
            iz: SecondMoment::from_si(4e-5 / divisor),
            torsion: SecondMoment::from_si(1e-5 / divisor),
        });
    }
    let n = 20;
    for i in 0..=n {
        let p = [Length::from_si(i as f64), Length::ZERO, Length::ZERO];
        m.add_node(if i == 0 { Node::fixed(p) } else { Node::new(p) });
        if i > 0 {
            m.add_frame(Frame::new(
                [NodeId(i - 1), NodeId(i)],
                MaterialId(0),
                SectionId(i % 2),
            ));
        }
    }
    m.add_load_case(LoadCase {
        name: "tip".into(),
        nodal: vec![NodalLoad::force(
            NodeId(n),
            [
                Force::from_si(1.0),
                Force::from_si(-1.0),
                Force::from_si(1.0),
            ],
        )],
        ..Default::default()
    });
    m
}

#[test]
fn refinement_recovers_a_stiffness_ratio_of_a_million() {
    let ratio = 1e6;
    let m = contrast_chain(ratio);
    let r = analyze_static(&m, &Default::default()).unwrap();
    let c = &r.combinations[0];
    assert!(c.relative_residual <= 1e-7, "{:e}", c.relative_residual);
    // Axial displacement of a series chain: sum of P L / (E A) over segments.
    let e = 200e9;
    let expected: f64 = (1..=20)
        .map(|i| 1.0 / (e * if i % 2 == 0 { 0.01 } else { 0.01 / ratio }))
        .sum();
    let actual = c.displacements.as_ref().unwrap()[20][0];
    // Forward error is bounded by condition number times machine epsilon.
    // A 1e6 stiffness ratio in bending and axial coupling leaves about eight
    // digits, which is what the backward-error criterion promises.
    assert!(
        (actual / expected - 1.0).abs() < 1e-6,
        "axial {actual:e} vs {expected:e}"
    );
    // Reactions carry the same conditioning-limited accuracy as the
    // displacements: about eight digits on the axial path, fewer on the
    // bending path where the stiffness contrast actually lives.
    let reaction = c.reactions.as_ref().unwrap()[0];
    assert!(
        (reaction[0] + 1.0).abs() < 1e-6,
        "axial reaction {:e}",
        reaction[0]
    );
    assert!(
        (reaction[1] - 1.0).abs() < 1e-3,
        "lateral reaction {:e}",
        reaction[1]
    );
}

#[test]
fn extreme_conditioning_is_still_rejected_rather_than_returned_wrong() {
    let m = contrast_chain(1e8);
    assert!(matches!(
        analyze_static(&m, &Default::default()),
        Err(Error::Unstable(_)) | Err(Error::Solver(_))
    ));
}
