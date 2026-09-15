//! Euler-Bernoulli space frame. DOFs: [ux, uy, uz, rx, ry, rz] at each end.
//! Consistent loads are integrated from displacement and rotation shape functions.
use super::{GAUSS3, block_rotation, rotation};
use crate::{
    Error, Result,
    model::*,
    units::{Length, LineLoad},
};
use nalgebra::{DMatrix, DVector, Matrix3, SMatrix, SVector, Vector3};
use std::ops::Deref;

pub(crate) type M12 = SMatrix<f64, 12, 12>;
pub(crate) type V12 = SVector<f64, 12>;

#[derive(Clone)]
pub(crate) struct FrameElement {
    pub dofs: [usize; 12],
    pub length: f64,
    pub r: Matrix3<f64>,
    pub t: M12,
    pub elastic: M12,
    pub geometric_unit: M12,
    pub releases: [bool; 12],
    pub mass: f64,
}

impl FrameElement {
    pub fn new(model: &Model, id: usize) -> Result<Self> {
        let frame = &model.frames[id];
        let m = &model.materials[frame.material.0];
        let s = &model.sections[frame.section.0];
        let a = Vector3::from(model.nodes[frame.nodes[0].0].xyz());
        let b = Vector3::from(model.nodes[frame.nodes[1].0].xyz());
        let length = (b - a).norm();
        let x = (b - a) / length;
        let hint = frame.local_y.map(Vector3::from).unwrap_or_else(|| {
            if x.x.hypot(x.z) > 1e-10 {
                Vector3::y()
            } else {
                Vector3::new(-x.y.signum(), 0.0, 0.0)
            }
        });
        let mut y = hint - x * x.dot(&hint);
        if y.norm() < 1e-10 * hint.norm().max(1.0) {
            return Err(Error::Model(format!(
                "frame {id}: local_y is parallel to the member or zero"
            )));
        }
        y.normalize_mut();
        y = y * frame.roll.si().cos() + x.cross(&y) * frame.roll.si().sin();
        let r = rotation(x, y);
        let t = block_rotation(&r);
        let mut k = M12::zeros();
        pair(&mut k, 0, 6, m.young.si() * s.area.si() / length);
        pair(&mut k, 3, 9, m.shear_modulus() * s.torsion.si() / length);
        bending(&mut k, [1, 5, 7, 11], m.young.si() * s.iz.si(), length, 1.0);
        bending(
            &mut k,
            [2, 4, 8, 10],
            m.young.si() * s.iy.si(),
            length,
            -1.0,
        );
        let mut kg = M12::zeros();
        // Positive N is tension; compression reduces transverse stiffness.
        geometric(&mut kg, [1, 5, 7, 11], length, 1.0);
        geometric(&mut kg, [2, 4, 8, 10], length, -1.0);
        pair(
            &mut kg,
            3,
            9,
            (s.iy.si() + s.iz.si()) / (s.area.si() * length),
        );
        Ok(Self {
            dofs: std::array::from_fn(|i| frame.nodes[i / 6].0 * 6 + i % 6),
            length,
            r,
            t,
            elastic: k,
            geometric_unit: kg,
            releases: frame.releases,
            mass: m.density.si() * s.area.si() * length,
        })
    }
    /// Uniform global-axis line load from density, area and gravity. None when zero.
    pub fn self_weight_load(
        &self,
        member: FrameId,
        gravity: f64,
        factors: [f64; 3],
    ) -> Option<MemberLoad> {
        let weight_per_length = self.mass / self.length * gravity;
        let q = factors.map(|f| LineLoad::from_si(f * weight_per_length));
        if q.iter().all(|v| v.si() == 0.0) {
            return None;
        }
        Some(MemberLoad::Distributed {
            member,
            start: Length::ZERO,
            end: Length::from_si(self.length),
            start_load: q,
            end_load: q,
            axes: Axes::Global,
        })
    }
    pub fn local_vector(&self, axes: Axes, v: [f64; 3]) -> Vector3<f64> {
        let v = Vector3::from(v);
        if axes == Axes::Global { self.r * v } else { v }
    }
    pub fn equivalent_load(&self, load: &MemberLoad) -> V12 {
        match load {
            MemberLoad::Point {
                position,
                force,
                moment,
                axes,
                ..
            } => {
                let f = self.local_vector(*axes, force.map(|v| v.si()));
                let m = self.local_vector(*axes, moment.map(|v| v.si()));
                self.load_at(position.si(), f, m)
            }
            MemberLoad::Distributed {
                start,
                end,
                start_load,
                end_load,
                axes,
                ..
            } => {
                let a = start.si();
                let b = end.si();
                let q0 = self.local_vector(*axes, start_load.map(|v| v.si()));
                let q1 = self.local_vector(*axes, end_load.map(|v| v.si()));
                let mut p = V12::zeros();
                for (xi, w) in GAUSS3 {
                    let t = (xi + 1.0) / 2.0;
                    p += self.load_at(a + t * (b - a), q0 + (q1 - q0) * t, Vector3::zeros())
                        * (w * (b - a) / 2.0);
                }
                p
            }
        }
    }
    fn load_at(&self, x: f64, f: Vector3<f64>, m: Vector3<f64>) -> V12 {
        let t = x / self.length;
        let l = self.length;
        let h = [
            1.0 - 3.0 * t * t + 2.0 * t * t * t,
            l * (t - 2.0 * t * t + t * t * t),
            3.0 * t * t - 2.0 * t * t * t,
            l * (-t * t + t * t * t),
        ];
        let dh = [
            (-6.0 * t + 6.0 * t * t) / l,
            1.0 - 4.0 * t + 3.0 * t * t,
            (6.0 * t - 6.0 * t * t) / l,
            -2.0 * t + 3.0 * t * t,
        ];
        let mut p = V12::zeros();
        p[0] = (1.0 - t) * f.x;
        p[6] = t * f.x;
        p[3] = (1.0 - t) * m.x;
        p[9] = t * m.x;
        for (j, id) in [1, 5, 7, 11].into_iter().enumerate() {
            p[id] += h[j] * f.y + dh[j] * m.z;
        }
        for (j, id) in [2, 4, 8, 10].into_iter().enumerate() {
            let sign = if j % 2 == 0 { 1.0 } else { -1.0 };
            p[id] += sign * (h[j] * f.z - dh[j] * m.y);
        }
        p
    }
    /// Stiffness at the given axial force, with released DOFs condensed out.
    /// Independent of loading, so linear analysis computes it once per member.
    pub fn stiffness(&self, axial: f64) -> FrameStiffness {
        let k = self.elastic + self.geometric_unit * axial;
        let released: Vec<_> = (0..12).filter(|i| self.releases[*i]).collect();
        let mut inv = DMatrix::<f64>::zeros(released.len(), released.len());
        if !released.is_empty() {
            let rr = DMatrix::from_fn(released.len(), released.len(), |i, j| {
                k[(released[i], released[j])]
            });
            let eig = rr.symmetric_eigen();
            let scale = eig.eigenvalues.amax();
            for a in 0..released.len() {
                let val = eig.eigenvalues[a];
                if val.abs() > scale * 1e-12 {
                    inv +=
                        (eig.eigenvectors.column(a) * eig.eigenvectors.column(a).transpose()) / val;
                }
            }
        }
        let mut kc = k;
        for i in 0..12 {
            for j in 0..12 {
                if self.releases[i] || self.releases[j] {
                    kc[(i, j)] = 0.0;
                } else {
                    for (a, &ra) in released.iter().enumerate() {
                        for (b, &rb) in released.iter().enumerate() {
                            kc[(i, j)] -= k[(i, ra)] * inv[(a, b)] * k[(rb, j)];
                        }
                    }
                }
            }
        }
        FrameStiffness {
            k,
            global_k: self.t.transpose() * kc * self.t,
            released,
            inv,
        }
    }
    /// Condenses the local fixed-end load through the release operators of
    /// `stiffness`, which must have been built for this member.
    pub fn state<'a>(&self, stiffness: Stiffness<'a>, p: V12) -> Result<FrameState<'a>> {
        let released = &stiffness.released;
        let k = &stiffness.k;
        let inv = &stiffness.inv;
        if !released.is_empty() {
            let pr = DVector::from_fn(released.len(), |i, _| p[released[i]]);
            let rr = DMatrix::from_fn(released.len(), released.len(), |i, j| {
                k[(released[i], released[j])]
            });
            if (&rr * inv * &pr - &pr).norm() > 1e-8 * pr.norm().max(1.0) {
                return Err(Error::Model(
                    "member load acts on an unsupported released member mode".into(),
                ));
            }
        }
        let mut pc = p;
        for i in 0..12 {
            if self.releases[i] {
                pc[i] = 0.0;
            } else {
                for (a, &ra) in released.iter().enumerate() {
                    for (b, &rb) in released.iter().enumerate() {
                        pc[i] -= k[(i, ra)] * inv[(a, b)] * p[rb];
                    }
                }
            }
        }
        Ok(FrameState {
            global_p: self.t.transpose() * pc,
            stiffness,
            p,
        })
    }
}

/// Load-independent member stiffness: local and condensed global matrices plus
/// the release recovery operator.
pub(crate) struct FrameStiffness {
    pub k: M12,
    pub global_k: M12,
    released: Vec<usize>,
    inv: DMatrix<f64>,
}

/// Shared cached stiffness, or an owned one for a nonzero axial force. Kept
/// pointer-sized so a per-combination state vector stays small.
pub(crate) enum Stiffness<'a> {
    Shared(&'a FrameStiffness),
    Owned(Box<FrameStiffness>),
}
impl Deref for Stiffness<'_> {
    type Target = FrameStiffness;
    fn deref(&self) -> &FrameStiffness {
        match self {
            Self::Shared(s) => s,
            Self::Owned(s) => s,
        }
    }
}

/// Member stiffness together with the condensed load for one combination.
pub(crate) struct FrameState<'a> {
    pub stiffness: Stiffness<'a>,
    pub p: V12,
    pub global_p: V12,
}
impl FrameState<'_> {
    pub fn global_k(&self) -> &M12 {
        &self.stiffness.global_k
    }
    pub fn recover(&self, element: &FrameElement, global_d: &[f64]) -> (V12, V12) {
        let FrameStiffness {
            k, released, inv, ..
        } = &*self.stiffness;
        let mut d = element.t * V12::from_fn(|i, _| global_d[element.dofs[i]]);
        if !released.is_empty() {
            let rhs = DVector::from_fn(released.len(), |a, _| {
                let ra = released[a];
                self.p[ra]
                    - (0..12)
                        .filter(|j| !element.releases[*j])
                        .map(|j| k[(ra, j)] * d[j])
                        .sum::<f64>()
            });
            let dr = inv * rhs;
            for (i, &r) in released.iter().enumerate() {
                d[r] = dr[i];
            }
        }
        let mut f = k * d - self.p;
        for &r in released {
            f[r] = 0.0;
        }
        (d, f)
    }
}

fn pair(k: &mut M12, a: usize, b: usize, v: f64) {
    k[(a, a)] += v;
    k[(b, b)] += v;
    k[(a, b)] -= v;
    k[(b, a)] -= v;
}
fn bending(k: &mut M12, ids: [usize; 4], ei: f64, l: f64, sign: f64) {
    let v = [
        [12.0, 6.0 * l, -12.0, 6.0 * l],
        [6.0 * l, 4.0 * l * l, -6.0 * l, 2.0 * l * l],
        [-12.0, -6.0 * l, 12.0, -6.0 * l],
        [6.0 * l, 2.0 * l * l, -6.0 * l, 4.0 * l * l],
    ];
    for i in 0..4 {
        for j in 0..4 {
            k[(ids[i], ids[j])] += v[i][j] * ei / l.powi(3)
                * (if i % 2 == 1 { sign } else { 1.0 })
                * (if j % 2 == 1 { sign } else { 1.0 });
        }
    }
}
fn geometric(k: &mut M12, ids: [usize; 4], l: f64, sign: f64) {
    let v = [
        [36.0, 3.0 * l, -36.0, 3.0 * l],
        [3.0 * l, 4.0 * l * l, -3.0 * l, -l * l],
        [-36.0, -3.0 * l, 36.0, -3.0 * l],
        [3.0 * l, -l * l, -3.0 * l, 4.0 * l * l],
    ];
    for i in 0..4 {
        for j in 0..4 {
            k[(ids[i], ids[j])] += v[i][j] / (30.0 * l)
                * (if i % 2 == 1 { sign } else { 1.0 })
                * (if j % 2 == 1 { sign } else { 1.0 });
        }
    }
}
