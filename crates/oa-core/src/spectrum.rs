//! Linear response-spectrum analysis using mass-normalized elastic modes.
//! Results are statistical component peaks, not a simultaneous signed state.
use crate::{
    analysis::{node_values, recover_frames},
    assembly::Prepared,
    exec::Exec,
    modal::{FaerKrylovSchur, analyze_modal_prepared},
    units::Acceleration,
    *,
};
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModalCombination {
    Srss,
    #[default]
    Cqc,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpectrumPoint {
    pub period_seconds: f64,
    pub acceleration: Acceleration,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpectrumOptions {
    pub modal: ModalOptions,
    pub spectrum: Vec<SpectrumPoint>,
    /// Nonzero global translation direction; normalized by the solver.
    pub direction: [f64; 3],
    /// Damping ratio used for CQC. The supplied spectrum must match this damping.
    pub damping: f64,
    pub combination: ModalCombination,
    pub minimum_mass_ratio: Option<f64>,
    pub outputs: OutputSelection,
}
impl Default for SpectrumOptions {
    fn default() -> Self {
        Self {
            modal: ModalOptions::default(),
            spectrum: vec![],
            direction: [1.0, 0.0, 0.0],
            damping: 0.05,
            combination: ModalCombination::Cqc,
            minimum_mass_ratio: None,
            outputs: OutputSelection::default(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectrumResult {
    pub modal: ModalResult,
    pub direction: [f64; 3],
    pub captured_mass_ratio: f64,
    pub combination: ModalCombination,
    pub displacements: Option<Vec<[f64; 6]>>,
    pub reactions: Option<Vec<[f64; 6]>>,
    pub frame_end_forces: Option<Vec<[f64; 12]>>,
    pub shells: Option<Vec<ShellResult>>,
    /// Summed support reaction per mode, then combined; moments about global origin.
    pub base_reaction: [f64; 6],
}

pub fn analyze_spectrum(model: &Model, options: &SpectrumOptions) -> Result<SpectrumResult> {
    Exec::new(options.modal.threads)?.install(|| spectrum_in_pool(model, options))
}
fn spectrum_in_pool(model: &Model, options: &SpectrumOptions) -> Result<SpectrumResult> {
    if options.spectrum.len() < 2
        || !options.damping.is_finite()
        || options.damping < 0.0
        || options.damping >= 1.0
        || options.direction.iter().any(|v| !v.is_finite())
    {
        return Err(Error::Request(
            "invalid spectrum, damping, or direction".into(),
        ));
    }
    if options
        .minimum_mass_ratio
        .is_some_and(|v| !v.is_finite() || v <= 0.0 || v > 1.0)
    {
        return Err(Error::Request("minimum_mass_ratio must be in (0,1]".into()));
    }
    let norm = options.direction.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm <= 0.0 || !norm.is_finite() {
        return Err(Error::Request(
            "spectrum direction must be finite and nonzero".into(),
        ));
    }
    let direction = options.direction.map(|v| v / norm);
    for (i, p) in options.spectrum.iter().enumerate() {
        if !p.period_seconds.is_finite()
            || p.period_seconds < 0.0
            || !p.acceleration.si().is_finite()
            || p.acceleration.si() < 0.0
            || (i > 0 && p.period_seconds <= options.spectrum[i - 1].period_seconds)
        {
            return Err(Error::Request("spectrum periods must increase strictly; periods and accelerations must be nonnegative and finite".into()));
        }
    }
    // One preparation serves the modal solve and the response recovery.
    let prep = Prepared::new(model)?;
    let modal = analyze_modal_prepared(model, &prep, &options.modal, &FaerKrylovSchur)?;
    let total = (0..3)
        .map(|a| direction[a] * direction[a] * modal.total_free_mass[a])
        .sum::<f64>();
    if total <= 0.0 {
        return Err(Error::Request(
            "no participating mass in spectrum direction".into(),
        ));
    }
    let gamma: Vec<_> = modal
        .modes
        .iter()
        .map(|m| {
            (0..3)
                .map(|a| direction[a] * m.participation[a])
                .sum::<f64>()
        })
        .collect();
    let captured = gamma.iter().map(|v| v * v).sum::<f64>() / total;
    if options
        .minimum_mass_ratio
        .is_some_and(|minimum| captured + 1e-10 < minimum)
    {
        return Err(Error::Request(format!(
            "captured mass ratio {captured:.6} is below the requested minimum; calculate more modes"
        )));
    }
    let states = prep.elastic_states()?;
    // No second factorization: element force recovery suffices for support reactions.
    let mass = prep.mass(model);
    let restrained: Vec<bool> = model.nodes.iter().flat_map(|n| n.restrained).collect();
    // Each mode's response is independent; modes are recovered in parallel
    // and kept in mode order for the combination.
    let respond = |j: usize| -> Result<Vec<f64>> {
        let mode = &modal.modes[j];
        let sa = interpolate(&options.spectrum, mode.period_seconds)?;
        let amplitude = gamma[j] * sa / mode.eigenvalue;
        let u: Vec<_> = mode
            .shape
            .iter()
            .flat_map(|n| n.iter().map(|v| v * amplitude))
            .collect();
        let frames = recover_frames(&prep, &states, &u);
        let shells = prep
            .shells
            .iter()
            .map(|e| e.recover(&u, 0.0))
            .collect::<Result<Vec<_>>>()?;
        let mut reaction = vec![0.0; prep.ndof];
        for ((e, s), f) in prep.frames.iter().zip(&states).zip(&frames) {
            if s.is_some() {
                let local = crate::element::frame::V12::from_row_slice(&f.local_end_forces);
                let global = e.t.transpose() * local;
                for i in 0..12 {
                    reaction[e.dofs[i]] += global[i];
                }
            }
        }
        for (e, s) in prep.shells.iter().zip(&shells) {
            let local = crate::element::shell::V24::from_row_slice(&s.local_end_forces);
            let global = e.t.transpose() * local;
            for i in 0..24 {
                reaction[e.dofs[i]] += global[i];
            }
        }
        // A slave DOF's element sum is K u there, which equals the modal
        // inertia of its own lumped mass plus the constraint force. Transfer
        // that constraint force to restrained masters through Tᵀ, so a
        // restrained diaphragm master reports the forces its slaves feed it.
        for (slave, terms) in prep.constraints.iter().enumerate() {
            let Some(terms) = terms else {
                continue;
            };
            if restrained[slave] {
                continue;
            }
            let constraint_force = reaction[slave] - mode.eigenvalue * mass[slave] * u[slave];
            for &(master, c) in terms {
                if restrained[master] {
                    reaction[master] += c * constraint_force;
                }
            }
        }
        let mut base = [0.0; 6];
        for (i, node) in model.nodes.iter().enumerate() {
            let springs = node.springs();
            for d in 0..6 {
                if springs[d] > 0.0 {
                    // Element sums include inertia at sprung nodes; use the spring law.
                    reaction[6 * i + d] = -springs[d] * u[6 * i + d];
                } else if !node.restrained[d] {
                    reaction[6 * i + d] = 0.0;
                }
                base[d] += reaction[6 * i + d];
            }
            let f =
                nalgebra::Vector3::new(reaction[6 * i], reaction[6 * i + 1], reaction[6 * i + 2]);
            let m = nalgebra::Vector3::from(node.xyz()).cross(&f);
            for a in 0..3 {
                base[3 + a] += m[a];
            }
        }
        let mut fields = base.to_vec();
        if options.outputs.displacements {
            fields.extend_from_slice(&u);
        }
        if options.outputs.reactions {
            fields.extend_from_slice(&reaction);
        }
        if options.outputs.frames {
            for f in &frames {
                fields.extend_from_slice(&f.local_end_forces);
            }
        }
        if options.outputs.shells {
            for s in &shells {
                fields.extend_from_slice(&s.local_end_forces);
                fields.extend_from_slice(&s.membrane_stress);
                fields.extend_from_slice(&s.bending_moment);
                fields.extend_from_slice(&s.transverse_shear);
            }
        }
        Ok(fields)
    };
    let count = modal.modes.len();
    #[cfg(feature = "parallel")]
    let responses = (0..count)
        .into_par_iter()
        .map(respond)
        .collect::<Result<Vec<_>>>()?;
    #[cfg(not(feature = "parallel"))]
    let responses = (0..count).map(respond).collect::<Result<Vec<_>>>()?;
    let correlation = match options.combination {
        ModalCombination::Srss => None,
        ModalCombination::Cqc => Some(
            (0..count)
                .map(|i| {
                    (0..count)
                        .map(|j| {
                            if i == j {
                                1.0
                            } else {
                                cqc_correlation(
                                    modal.modes[i].frequency_hz,
                                    modal.modes[j].frequency_hz,
                                    options.damping,
                                )
                            }
                        })
                        .collect()
                })
                .collect::<Vec<Vec<f64>>>(),
        ),
    };
    let combined = combine(&responses, correlation.as_deref())?;
    let mut offset = 6;
    let base_reaction = std::array::from_fn(|i| combined[i]);
    let displacements = if options.outputs.displacements {
        let out = node_values(&combined[offset..offset + prep.ndof]);
        offset += prep.ndof;
        Some(out)
    } else {
        None
    };
    let reactions = if options.outputs.reactions {
        let out = node_values(&combined[offset..offset + prep.ndof]);
        offset += prep.ndof;
        Some(out)
    } else {
        None
    };
    let frame_end_forces = if options.outputs.frames {
        Some(
            (0..prep.frames.len())
                .map(|_| {
                    let f = std::array::from_fn(|i| combined[offset + i]);
                    offset += 12;
                    f
                })
                .collect(),
        )
    } else {
        None
    };
    let shells = if options.outputs.shells {
        Some(
            (0..prep.shells.len())
                .map(|_| {
                    let local_end_forces = std::array::from_fn(|i| combined[offset + i]);
                    offset += 24;
                    let membrane_stress = std::array::from_fn(|i| combined[offset + i]);
                    offset += 3;
                    let bending_moment = std::array::from_fn(|i| combined[offset + i]);
                    offset += 3;
                    let transverse_shear = std::array::from_fn(|i| combined[offset + i]);
                    offset += 2;
                    ShellResult {
                        local_end_forces,
                        membrane_stress,
                        bending_moment,
                        transverse_shear,
                    }
                })
                .collect(),
        )
    } else {
        None
    };
    Ok(SpectrumResult {
        modal,
        direction,
        captured_mass_ratio: captured,
        combination: options.combination,
        displacements,
        reactions,
        frame_end_forces,
        shells,
        base_reaction,
    })
}
fn interpolate(points: &[SpectrumPoint], period: f64) -> Result<f64> {
    if period < points[0].period_seconds || period > points.last().unwrap().period_seconds {
        return Err(Error::Request(format!(
            "spectrum does not cover modal period {period:.6} s"
        )));
    }
    let j = points.partition_point(|p| p.period_seconds < period).max(1);
    let a = &points[j - 1];
    let b = &points[j];
    Ok(a.acceleration.si()
        + (b.acceleration.si() - a.acceleration.si()) * (period - a.period_seconds)
            / (b.period_seconds - a.period_seconds))
}
fn cqc_correlation(a: f64, b: f64, z: f64) -> f64 {
    if z == 0.0 {
        return 0.0;
    }
    let r = a.min(b) / a.max(b);
    let numerator = 8.0 * z * z * (1.0 + r) * r.powf(1.5);
    numerator / ((1.0 - r * r).powi(2) + 4.0 * z * z * r * (1.0 + r).powi(2))
}
/// Quadratic modal combination per output component. `None` correlation is
/// SRSS, whose cross terms vanish, so it needs only the diagonal sum.
/// Components are independent, so they are tiled across workers; within a
/// component the summation order is fixed.
fn combine(responses: &[Vec<f64>], correlation: Option<&[Vec<f64>]>) -> Result<Vec<f64>> {
    let one = |k: usize| -> Result<f64> {
        let scale = responses.iter().map(|r| r[k].powi(2)).sum::<f64>();
        let sum = match correlation {
            None => scale,
            Some(rho) => {
                let mut sum = 0.0;
                for i in 0..responses.len() {
                    for j in 0..responses.len() {
                        sum += rho[i][j] * responses[i][k] * responses[j][k];
                    }
                }
                sum
            }
        };
        if !sum.is_finite() || sum < -1e-10 * scale {
            Err(Error::Solver(
                "invalid response-spectrum quadratic combination".into(),
            ))
        } else {
            Ok(sum.max(0.0).sqrt())
        }
    };
    let n = responses[0].len();
    #[cfg(feature = "parallel")]
    if n >= crate::exec::PARALLEL_THRESHOLD {
        return (0..n).into_par_iter().map(one).collect();
    }
    (0..n).map(one).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cqc_repeated_modes_and_signed_cross_terms() {
        let rho = cqc_correlation(5.0, 5.0, 0.05);
        assert!((rho - 1.0).abs() < 1e-14);
        assert!(cqc_correlation(1.0, 100.0, 0.05) < 0.001);
        let v = combine(
            &[vec![3.0], vec![-2.0]],
            Some(&[vec![1.0, 1.0], vec![1.0, 1.0]]),
        )
        .unwrap();
        assert!((v[0] - 1.0).abs() < 1e-14);
    }
    #[test]
    fn srss_matches_identity_correlation() {
        let responses = vec![
            vec![3.0, -1.0, 0.5],
            vec![-2.0, 4.0, 0.0],
            vec![1.0, 1.0, -2.0],
        ];
        let identity: Vec<Vec<f64>> = (0..3)
            .map(|i| (0..3).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
            .collect();
        let srss = combine(&responses, None).unwrap();
        let generic = combine(&responses, Some(&identity)).unwrap();
        for (a, b) in srss.iter().zip(&generic) {
            assert!((a - b).abs() <= 1e-14 * a.abs());
        }
        assert!((srss[0] - 14.0_f64.sqrt()).abs() < 1e-14);
    }
}
