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
    section_forces(&e, model, combination, member, result, x.si())
}

/// `frame_section_forces` on a member already checked and built.
fn section_forces(
    e: &FrameElement,
    model: &Model,
    combination: &LoadCombination,
    member: FrameId,
    result: &FrameResult,
    x: f64,
) -> Result<SectionForces> {
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
        let self_weight = e.self_weight_load(member, model.gravity.si(), case.self_weight);
        for load in case
            .member
            .iter()
            .filter(|l| l.member() == member)
            .chain(self_weight.iter())
        {
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

/// Section forces and transverse deflections at evenly spaced stations along
/// a member, for drawing its diagrams.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameDiagram {
    /// Local axes as unit vectors in global coordinates: x from I to J, then y and z.
    pub axes: [[f64; 3]; 3],
    /// Member length, metres.
    pub length: f64,
    /// Distance from end I of each station, metres, from 0 to the length.
    pub stations: Vec<f64>,
    /// N, Vy, Vz, T, My, Mz at each station, as `frame_section_forces` reports them.
    pub forces: Vec<[f64; 6]>,
    /// Deflection along local y and z at each station, metres: the end
    /// displacements carried along the member by integrating its curvature.
    pub deflections: Vec<[f64; 2]>,
}

/// Exact first-order section forces at `stations` evenly spaced points (at
/// least two), and the Euler-Bernoulli deflection integrated from them. At a
/// point load the station takes the right-hand limit.
pub fn frame_diagram(
    model: &Model,
    combination: &LoadCombination,
    member: FrameId,
    result: &FrameResult,
    stations: usize,
) -> Result<FrameDiagram> {
    model.validate()?;
    if member.0 >= model.frames.len() {
        return Err(Error::Request("invalid member ID".into()));
    }
    if stations < 2 {
        return Err(Error::Request("a diagram needs at least two stations".into()));
    }
    let e = FrameElement::new(model, member.0)?;
    let frame = &model.frames[member.0];
    let young = model.materials[frame.material.0].young.si();
    let section = &model.sections[frame.section.0];
    let (ei_y, ei_z) = (young * section.iy.si(), young * section.iz.si());
    let xs: Vec<f64> = (0..stations)
        .map(|k| e.length * k as f64 / (stations - 1) as f64)
        .collect();
    let mut forces = Vec::with_capacity(stations);
    for &x in &xs {
        forces.push(section_forces(&e, model, combination, member, result, x)?.values);
    }
    // Sagging in the x-y plane is +Mz, so v'' = Mz / EIz; sagging in the x-z
    // plane is -My, so w'' = -My / EIy. The slopes start from the end
    // rotations: v' = rz and w' = -ry. Curvature is taken as linear between
    // stations, which is exact under point loads.
    let d = result.local_displacements;
    let (mut v, mut dv, mut w, mut dw) = (d[1], d[5], d[2], -d[4]);
    let mut deflections = Vec::with_capacity(stations);
    deflections.push([v, w]);
    for k in 1..stations {
        let h = xs[k] - xs[k - 1];
        let (kz0, kz1) = (forces[k - 1][5] / ei_z, forces[k][5] / ei_z);
        let (ky0, ky1) = (-forces[k - 1][4] / ei_y, -forces[k][4] / ei_y);
        v += dv * h + h * h * (kz0 / 3.0 + kz1 / 6.0);
        dv += h * (kz0 + kz1) / 2.0;
        w += dw * h + h * h * (ky0 / 3.0 + ky1 / 6.0);
        dw += h * (ky0 + ky1) / 2.0;
        deflections.push([v, w]);
    }
    Ok(FrameDiagram {
        axes: std::array::from_fn(|i| [e.r[(i, 0)], e.r[(i, 1)], e.r[(i, 2)]]),
        length: e.length,
        stations: xs,
        forces,
        deflections,
    })
}
