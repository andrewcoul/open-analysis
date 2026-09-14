use crate::{Error, Result, element::frame::FrameElement, model::*, units::Length};
use nalgebra::Vector3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameResult {
    pub active: bool,
    /// Forces applied to the element at its ends, in local axes (N and N m).
    pub local_end_forces: [f64; 12],
    /// Includes recovered hinge rotations / released displacements.
    pub local_displacements: [f64; 12],
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellResult {
    pub local_end_forces: [f64; 24],
    /// [sigma_x, sigma_y, tau_xy] at the element center, Pa.
    pub membrane_stress: [f64; 3],
    /// [Mx, My, Mxy] at the element center, N m / m.
    pub bending_moment: [f64; 3],
    /// [Qx, Qy] at the element center, N / m. Kirchhoff rectangles return zero.
    pub transverse_shear: [f64; 2],
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CombinationResult {
    pub combination: String,
    pub iterations: usize,
    pub relative_residual: f64,
    /// Full node order; [ux,uy,uz,rx,ry,rz], SI. None when deselected.
    pub displacements: Option<Vec<[f64; 6]>>,
    /// Support forces and moments; zeros at unrestrained DOFs.
    pub reactions: Option<Vec<[f64; 6]>>,
    pub frames: Option<Vec<FrameResult>>,
    pub shells: Option<Vec<ShellResult>>,
}

/// Called serially, in requested combination order. An error stops analysis.
pub trait ResultConsumer {
    fn consume(&mut self, result: CombinationResult) -> Result<()>;
}
#[derive(Debug, Default)]
pub struct InMemoryResults {
    pub combinations: Vec<CombinationResult>,
}
impl ResultConsumer for InMemoryResults {
    fn consume(&mut self, result: CombinationResult) -> Result<()> {
        self.combinations.push(result);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoverningValue {
    pub value: f64,
    pub combination: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub minimum: GoverningValue,
    pub maximum: GoverningValue,
}
impl Envelope {
    pub fn from_values<'a>(values: impl IntoIterator<Item = (&'a str, f64)>) -> Result<Self> {
        let mut out: Option<Self> = None;
        for (name, value) in values {
            if !value.is_finite() {
                return Err(Error::Request("cannot envelope a nonfinite result".into()));
            }
            let v = GoverningValue {
                value,
                combination: name.into(),
            };
            if let Some(e) = &mut out {
                if value < e.minimum.value {
                    e.minimum = v.clone();
                }
                if value > e.maximum.value {
                    e.maximum = v;
                }
            } else {
                out = Some(Self {
                    minimum: v.clone(),
                    maximum: v,
                });
            }
        }
        out.ok_or_else(|| Error::Request("cannot envelope an empty result set".into()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionForces {
    pub x: f64,
    /// Traction on the positive-x face of the left cut: N, Vy, Vz, T, My, Mz.
    pub values: [f64; 6],
}

/// Exact first-order section equilibrium, including partial distributed and point
/// loads. At a point-load discontinuity this returns the right-hand limit.
pub fn frame_section_forces(
    model: &Model,
    combination: &LoadCombination,
    member: FrameId,
    result: &FrameResult,
    x: Length,
) -> Result<SectionForces> {
    model.validate()?;
    if member.0 >= model.frames.len() {
        return Err(Error::Request("invalid member ID".into()));
    }
    let e = FrameElement::new(model, member.0)?;
    let x = x.si();
    if !x.is_finite() || x < 0.0 || x > e.length {
        return Err(Error::Request("section position outside member".into()));
    }
    if !result.active {
        return Ok(SectionForces {
            x,
            values: [0.0; 6],
        });
    }
    let f = result.local_end_forces;
    let mut force = Vector3::new(f[0], f[1], f[2]);
    let mut moment = Vector3::new(f[3], f[4], f[5]) + Vector3::new(-x, 0.0, 0.0).cross(&force);
    for &(id, factor) in &combination.terms {
        let case = model
            .load_cases
            .get(id.0)
            .ok_or_else(|| Error::Request("invalid load case in section recovery".into()))?;
        if !factor.is_finite() {
            return Err(Error::Request("nonfinite combination factor".into()));
        }
        for load in &case.member {
            if load.member() != member {
                continue;
            }
            match load {
                MemberLoad::Point {
                    position,
                    force: f,
                    moment: m,
                    axes,
                    ..
                } if position.si() <= x => {
                    let f = e.local_vector(*axes, f.map(|v| v.si())) * factor;
                    force += f;
                    moment += e.local_vector(*axes, m.map(|v| v.si())) * factor
                        + Vector3::new(position.si() - x, 0.0, 0.0).cross(&f);
                }
                MemberLoad::Distributed {
                    start,
                    end,
                    start_load,
                    end_load,
                    axes,
                    ..
                } if start.si() < x => {
                    let a = start.si();
                    let b = end.si().min(x);
                    let span = end.si() - a;
                    let q0 = e.local_vector(*axes, start_load.map(|v| v.si())) * factor;
                    let slope =
                        (e.local_vector(*axes, end_load.map(|v| v.si())) * factor - q0) / span;
                    let h = b - a;
                    let f = q0 * h + slope * (h * h / 2.0);
                    let first = q0 * (h * h / 2.0) + slope * (h * h * h / 3.0) + f * (a - x);
                    force += f;
                    moment += Vector3::x().cross(&first);
                }
                _ => {}
            }
        }
    }
    Ok(SectionForces {
        x,
        values: [
            -force.x, -force.y, -force.z, -moment.x, -moment.y, -moment.z,
        ],
    })
}
