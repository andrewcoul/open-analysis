//! Euler-Bernoulli space frame. DOFs: [ux, uy, uz, rx, ry, rz] at each end.
//! Consistent loads are integrated from displacement and rotation shape functions.
use super::{GAUSS3, block_rotation, rotation};
use crate::{Error, Result, model::*};
use nalgebra::{DMatrix, DVector, Matrix3, SMatrix, SVector, Vector3};

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
    pub fn state(&self, axial: f64, p: V12) -> Result<FrameState> {
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
            let pr = DVector::from_fn(released.len(), |i, _| p[released[i]]);
            let rr = DMatrix::from_fn(released.len(), released.len(), |i, j| {
                k[(released[i], released[j])]
            });
            if (&rr * &inv * &pr - &pr).norm() > 1e-8 * pr.norm().max(1.0) {
                return Err(Error::Model(
                    "member load acts on an unsupported released member mode".into(),
                ));
            }
        }
        let mut kc = k;
        let mut pc = p;
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
            k,
            p,
            global_k: self.t.transpose() * kc * self.t,
            global_p: self.t.transpose() * pc,
            released,
            inv,
        })
    }
}

pub(crate) struct FrameState {
    pub k: M12,
    pub p: V12,
    pub global_k: M12,
    pub global_p: V12,
    released: Vec<usize>,
    inv: DMatrix<f64>,
}
impl FrameState {
    pub fn recover(&self, element: &FrameElement, global_d: &[f64]) -> (V12, V12) {
        let mut d = element.t * V12::from_fn(|i, _| global_d[element.dofs[i]]);
        if !self.released.is_empty() {
            let rhs = DVector::from_fn(self.released.len(), |a, _| {
                let ra = self.released[a];
                self.p[ra]
                    - (0..12)
                        .filter(|j| !element.releases[*j])
                        .map(|j| self.k[(ra, j)] * d[j])
                        .sum::<f64>()
            });
            let dr = &self.inv * rhs;
            for (i, &r) in self.released.iter().enumerate() {
                d[r] = dr[i];
            }
        }
        let mut f = self.k * d - self.p;
        for &r in &self.released {
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
