//! Space frame. DOFs: [ux, uy, uz, rx, ry, rz] at each end.
//! Each bending plane is a Timoshenko beam when the section gives a shear
//! area for it and an Euler-Bernoulli beam otherwise. The shear parameter
//! phi = 12EI / (G As L^2) is zero in the second case, and every matrix and
//! shape function below reduces to its Euler-Bernoulli form. Consistent loads
//! are integrated from displacement and rotation shape functions.
use super::{GAUSS3, block_rotation, rotation};
use crate::{
    Error, Result,
    model::*,
    units::{Area, Length, LineLoad},
};
use nalgebra::{DMatrix, DVector, Matrix3, SMatrix, SVector, Vector3};
use std::ops::Deref;

pub(crate) type M12 = SMatrix<f64, 12, 12>;
pub(crate) type V12 = SVector<f64, 12>;

/// A member's line in space: where it starts after its joint offset at I,
/// its length to the moved end J, and its local axes as rows.
pub(crate) struct FrameLine {
    pub start: Vector3<f64>,
    pub length: f64,
    pub r: Matrix3<f64>,
}
impl FrameLine {
    pub fn new(model: &Model, id: usize) -> Result<Self> {
        let frame = &model.frames[id];
        let nodes = frame.nodes.map(|n| model.nodes[n.0].xyz());
        let tagged = |e: String| Error::Model(format!("frame {id}: {e}"));
        let [a, b] = frame.ends(nodes).map_err(tagged)?.map(Vector3::from);
        Ok(Self {
            start: a,
            length: (b - a).norm(),
            r: Matrix3::from(frame.axes_along((b - a).into()).map_err(tagged)?).transpose(),
        })
    }
}

/// Local axes as rows for a member along `span`: local y from the frame's
/// hint, or global Y (global -X for a vertical member), turned by its roll.
pub(crate) fn orientation(
    frame: &Frame,
    span: Vector3<f64>,
) -> std::result::Result<Matrix3<f64>, String> {
    let length = span.norm();
    if !length.is_finite() || length <= 1e-12 {
        return Err("zero or invalid length".into());
    }
    let x = span / length;
    let hint = frame.local_y.map(Vector3::from).unwrap_or_else(|| {
        if x.x.hypot(x.z) > 1e-10 {
            Vector3::y()
        } else {
            Vector3::new(-x.y.signum(), 0.0, 0.0)
        }
    });
    let mut y = hint - x * x.dot(&hint);
    if y.norm() < 1e-10 * hint.norm().max(1.0) {
        return Err("local_y is parallel to the member or zero".into());
    }
    y.normalize_mut();
    y = y * frame.roll.si().cos() + x.cross(&y) * frame.roll.si().sin();
    Ok(rotation(x, y))
}

/// Carries a node's six DOFs to a point at `arm` from it on a rigid link:
/// the point moves u + theta x arm and turns with the node.
fn rigid_link(arm: Vector3<f64>) -> SMatrix<f64, 6, 6> {
    let mut a = SMatrix::<f64, 6, 6>::identity();
    let skew = arm.cross_matrix();
    for i in 0..3 {
        for j in 0..3 {
            a[(i, 3 + j)] = -skew[(i, j)];
        }
    }
    a
}

#[derive(Clone)]
pub(crate) struct FrameElement {
    pub dofs: [usize; 12],
    /// The whole member, end offsets included, from its moved end I to J.
    /// Load positions and section stations run along this length.
    pub length: f64,
    /// Where the flexible part starts along `length`, and its length. The
    /// rest is rigid end zone.
    pub flexible: [f64; 2],
    /// Start and end of the clear length along `length`, between the end
    /// offsets: where section forces are reported.
    pub clear: [f64; 2],
    /// Global position of the member's moved end I, where `length` starts.
    pub start: Vector3<f64>,
    pub r: Matrix3<f64>,
    /// Global node DOFs to local DOFs at the ends of the flexible part:
    /// rigid links through joint offsets and rigid zones, then rotation.
    pub t: M12,
    pub elastic: M12,
    pub geometric_unit: M12,
    pub releases: [bool; 12],
    /// Own mass times the mass modifier.
    pub mass: f64,
    /// Own mass times the weight modifier: the mass whose weight under
    /// gravity is the member's self-weight.
    pub weight_mass: f64,
    /// Shear parameter for bending in the local x-y plane, then x-z.
    phi: [f64; 2],
}

impl FrameElement {
    pub fn new(model: &Model, id: usize) -> Result<Self> {
        let frame = &model.frames[id];
        let m = &model.materials[frame.material.0];
        let s = &model.sections[frame.section.0];
        frame
            .offsets
            .check()
            .map_err(|e| Error::Model(format!("frame {id}: {e}")))?;
        let FrameLine {
            start,
            length: total,
            r,
        } = FrameLine::new(model, id)?;
        let [oi, oj] = frame.offsets.end.map(Length::si);
        if oi + oj >= total * (1.0 - 1e-9) {
            return Err(Error::Model(format!(
                "frame {id}: end offsets leave no clear length"
            )));
        }
        let rigid = frame.offsets.rigid_zone;
        let flexible = [rigid * oi, total - rigid * (oi + oj)];
        let length = flexible[1];
        let x = r.row(0).transpose();
        let ends = [start + x * flexible[0], start + x * (flexible[0] + length)];
        let mut link = M12::zeros();
        for (k, node) in frame.nodes.iter().enumerate() {
            let arm = ends[k] - Vector3::from(model.nodes[node.0].xyz());
            link.fixed_view_mut::<6, 6>(6 * k, 6 * k)
                .copy_from(&rigid_link(arm));
        }
        let t = block_rotation::<12>(&r) * link;
        // Stiffness modifiers scale the stiffness only; mass and the
        // geometric stiffness's polar radius of gyration keep the section's
        // values, and mass and weight have modifiers of their own.
        let md = &frame.modifiers;
        let (ei_z, ei_y) = (
            m.young.si() * s.iz.si() * md.iz,
            m.young.si() * s.iy.si() * md.iy,
        );
        let phi = |ei: f64, shear: Option<Area>, modifier: f64| {
            shear.map_or(0.0, |a| {
                12.0 * ei / (m.shear_modulus() * a.si() * modifier * length * length)
            })
        };
        let phi = [
            phi(ei_z, s.shear_y, md.shear_y),
            phi(ei_y, s.shear_z, md.shear_z),
        ];
        let mut k = M12::zeros();
        pair(&mut k, 0, 6, m.young.si() * s.area.si() * md.area / length);
        pair(
            &mut k,
            3,
            9,
            m.shear_modulus() * s.torsion.si() * md.torsion / length,
        );
        bending(&mut k, [1, 5, 7, 11], ei_z, length, 1.0, phi[0]);
        bending(&mut k, [2, 4, 8, 10], ei_y, length, -1.0, phi[1]);
        let mut kg = M12::zeros();
        // Positive N is tension; compression reduces transverse stiffness.
        geometric(&mut kg, [1, 5, 7, 11], length, 1.0, phi[0]);
        geometric(&mut kg, [2, 4, 8, 10], length, -1.0, phi[1]);
        pair(
            &mut kg,
            3,
            9,
            (s.iy.si() + s.iz.si()) / (s.area.si() * length),
        );
        Ok(Self {
            dofs: std::array::from_fn(|i| frame.nodes[i / 6].0 * 6 + i % 6),
            length: total,
            flexible,
            clear: [oi, total - oj],
            start,
            r,
            t,
            elastic: k,
            geometric_unit: kg,
            releases: frame.releases,
            mass: m.density.si() * s.area.si() * total * md.mass,
            weight_mass: m.density.si() * s.area.si() * total * md.weight,
            phi,
        })
    }
    /// Uniform global-axis line load from density, area, the weight modifier
    /// and gravity. None when zero.
    pub fn self_weight_load(
        &self,
        member: FrameId,
        gravity: f64,
        factors: [f64; 3],
    ) -> Option<MemberLoad> {
        let weight_per_length = self.weight_mass / self.length * gravity;
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
    /// The load's fixed-end forces on the flexible part, in local axes,
    /// and the global nodal loads of what lands in the rigid zones. A load
    /// on a rigid zone reaches its node without passing the member's
    /// releases, so it never enters the condensed member load.
    pub fn equivalent_load(&self, load: &MemberLoad) -> (V12, V12) {
        let (mut p, mut rigid) = (V12::zeros(), V12::zeros());
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
                self.load_at(position.si(), f, m, &mut p, &mut rigid);
            }
            MemberLoad::Distributed {
                start,
                end,
                start_load,
                end_load,
                axes,
                ..
            } => {
                let (a, b) = (start.si(), end.si());
                let q0 = self.local_vector(*axes, start_load.map(|v| v.si()));
                let q1 = self.local_vector(*axes, end_load.map(|v| v.si()));
                // Integrate each zone on its own: the transfer through a
                // rigid zone has a kink at the flexible part's end.
                let [f0, lf] = self.flexible;
                let mut cuts = vec![a, b];
                cuts.extend([f0, f0 + lf].into_iter().filter(|c| *c > a && *c < b));
                cuts.sort_by(f64::total_cmp);
                for piece in cuts.windows(2) {
                    let (c, d) = (piece[0], piece[1]);
                    for (xi, w) in GAUSS3 {
                        let x = c + (xi + 1.0) / 2.0 * (d - c);
                        let q = q0 + (q1 - q0) * ((x - a) / (b - a));
                        let w = w * (d - c) / 2.0;
                        self.load_at(x, q * w, Vector3::zeros(), &mut p, &mut rigid);
                    }
                }
            }
        }
        (p, self.t.transpose() * rigid)
    }
    /// The load's gravity (-Z) resultant split statically between the two
    /// ends, as a simply supported span would carry it. Moments carry none.
    pub fn gravity_at_ends(&self, load: &MemberLoad) -> [f64; 2] {
        let down = |axes: &Axes, v: Vector3<f64>| match axes {
            Axes::Global => -v.z,
            Axes::Local => -self.r.column(2).dot(&v),
        };
        let split = |x: f64, w: f64| [w * (1.0 - x / self.length), w * x / self.length];
        match load {
            MemberLoad::Point {
                position,
                force,
                axes,
                ..
            } => split(
                position.si(),
                down(axes, Vector3::from(force.map(|v| v.si()))),
            ),
            MemberLoad::Distributed {
                start,
                end,
                start_load,
                end_load,
                axes,
                ..
            } => {
                let (a, b) = (start.si(), end.si());
                let q0 = down(axes, Vector3::from(start_load.map(|v| v.si())));
                let q1 = down(axes, Vector3::from(end_load.map(|v| v.si())));
                let mut ends = [0.0; 2];
                for (xi, w) in GAUSS3 {
                    let t = (xi + 1.0) / 2.0;
                    let [i, j] = split(a + t * (b - a), (q0 + (q1 - q0) * t) * w * (b - a) / 2.0);
                    ends[0] += i;
                    ends[1] += j;
                }
                ends
            }
        }
    }
    /// Adds a force and moment at `x` along the member: to `p` as fixed-end
    /// forces when it acts on the flexible part, or to `rigid` at the end of
    /// the flexible part, with the moment of its lever arm, when it acts on
    /// a rigid zone.
    fn load_at(&self, x: f64, f: Vector3<f64>, m: Vector3<f64>, p: &mut V12, rigid: &mut V12) {
        let [f0, length] = self.flexible;
        let s = x - f0;
        let arm = if s < 0.0 {
            Some((0, s))
        } else if s > length {
            Some((6, s - length))
        } else {
            None
        };
        if let Some((end, arm)) = arm {
            let m = m + Vector3::new(arm, 0.0, 0.0).cross(&f);
            for i in 0..3 {
                rigid[end + i] += f[i];
                rigid[end + 3 + i] += m[i];
            }
            return;
        }
        let t = s / length;
        p[0] += (1.0 - t) * f.x;
        p[6] += t * f.x;
        p[3] += (1.0 - t) * m.x;
        p[9] += t * m.x;
        // A force works through the deflection, a moment through the section
        // rotation; with shear deformation the two differ.
        let (h, rot) = shape(t, length, self.phi[0]);
        for (j, id) in [1, 5, 7, 11].into_iter().enumerate() {
            p[id] += h[j] * f.y + rot[j] * m.z;
        }
        let (h, rot) = shape(t, length, self.phi[1]);
        for (j, id) in [2, 4, 8, 10].into_iter().enumerate() {
            let sign = if j % 2 == 0 { 1.0 } else { -1.0 };
            p[id] += sign * (h[j] * f.z - rot[j] * m.y);
        }
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
/// Deflection and section-rotation shape functions at t = x / L for the
/// [v_i, theta_i, v_j, theta_j] DOFs of one bending plane. These are exact
/// for a Timoshenko beam without span load, so consistent loads built from
/// them are the exact fixed-end forces.
fn shape(t: f64, l: f64, phi: f64) -> ([f64; 4], [f64; 4]) {
    let d = 1.0 + phi;
    let (t2, t3) = (t * t, t * t * t);
    let v = [
        (1.0 + phi - phi * t - 3.0 * t2 + 2.0 * t3) / d,
        l * ((1.0 + phi / 2.0) * t - (2.0 + phi / 2.0) * t2 + t3) / d,
        (phi * t + 3.0 * t2 - 2.0 * t3) / d,
        l * (-phi / 2.0 * t - (1.0 - phi / 2.0) * t2 + t3) / d,
    ];
    let rotation = [
        6.0 * (t2 - t) / (d * l),
        (1.0 + phi - (4.0 + phi) * t + 3.0 * t2) / d,
        -6.0 * (t2 - t) / (d * l),
        (-(2.0 - phi) * t + 3.0 * t2) / d,
    ];
    (v, rotation)
}
fn bending(k: &mut M12, ids: [usize; 4], ei: f64, l: f64, sign: f64, phi: f64) {
    let v = [
        [12.0, 6.0 * l, -12.0, 6.0 * l],
        [6.0 * l, (4.0 + phi) * l * l, -6.0 * l, (2.0 - phi) * l * l],
        [-12.0, -6.0 * l, 12.0, -6.0 * l],
        [6.0 * l, (2.0 - phi) * l * l, -6.0 * l, (4.0 + phi) * l * l],
    ];
    for i in 0..4 {
        for j in 0..4 {
            k[(ids[i], ids[j])] += v[i][j] * ei / (l.powi(3) * (1.0 + phi))
                * (if i % 2 == 1 { sign } else { 1.0 })
                * (if j % 2 == 1 { sign } else { 1.0 });
        }
    }
}
/// Geometric stiffness per unit axial force, the integral of the product of
/// deflection slopes. With shear deformation this is Engesser's model: a
/// pinned column buckles at Pe / (1 + Pe / (G As)).
fn geometric(k: &mut M12, ids: [usize; 4], l: f64, sign: f64, phi: f64) {
    let a = 36.0 + 60.0 * phi + 30.0 * phi * phi;
    let b = (4.0 + 5.0 * phi + 2.5 * phi * phi) * l * l;
    let c = -(1.0 + 5.0 * phi + 2.5 * phi * phi) * l * l;
    let v = [
        [a, 3.0 * l, -a, 3.0 * l],
        [3.0 * l, b, -3.0 * l, c],
        [-a, -3.0 * l, a, -3.0 * l],
        [3.0 * l, c, -3.0 * l, b],
    ];
    for i in 0..4 {
        for j in 0..4 {
            k[(ids[i], ids[j])] += v[i][j] / (30.0 * l * (1.0 + phi).powi(2))
                * (if i % 2 == 1 { sign } else { 1.0 })
                * (if j % 2 == 1 { sign } else { 1.0 });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GAUSS3, M12, bending, geometric, shape};

    /// The closed-form matrices must equal the energy integrals of the shape
    /// functions: bending plus shear strain for the elastic stiffness, and
    /// the product of deflection slopes for the geometric stiffness.
    #[test]
    fn matrices_match_shape_function_integrals() {
        let (ei, l) = (3.0, 2.0);
        let ids = [0, 1, 2, 3];
        for phi in [0.0, 0.4, 3.0] {
            let (mut k, mut kg) = (M12::zeros(), M12::zeros());
            bending(&mut k, ids, ei, l, 1.0, phi);
            geometric(&mut kg, ids, l, 1.0, phi);
            // Slopes by central difference; the shape functions are cubic.
            let h = 1e-5;
            let derivatives = |t: f64| {
                let (v0, r0) = shape(t - h, l, phi);
                let (v1, r1) = shape(t + h, l, phi);
                let dv: [f64; 4] = std::array::from_fn(|i| (v1[i] - v0[i]) / (2.0 * h * l));
                let dr: [f64; 4] = std::array::from_fn(|i| (r1[i] - r0[i]) / (2.0 * h * l));
                (dv, dr)
            };
            for i in 0..4 {
                for j in 0..4 {
                    let (mut elastic, mut slope) = (0.0, 0.0);
                    for (xi, w) in GAUSS3 {
                        let t = (xi + 1.0) / 2.0;
                        let (dv, dr) = derivatives(t);
                        let (_, r) = shape(t, l, phi);
                        let shear = |n: usize| dv[n] - r[n];
                        let ga = if phi > 0.0 {
                            12.0 * ei / (phi * l * l)
                        } else {
                            0.0
                        };
                        elastic += w * l / 2.0 * (ei * dr[i] * dr[j] + ga * shear(i) * shear(j));
                        slope += w * l / 2.0 * dv[i] * dv[j];
                    }
                    let scale = k[(i, i)].abs().max(k[(j, j)].abs());
                    assert!(
                        (k[(i, j)] - elastic).abs() < 1e-6 * scale,
                        "phi {phi} k[{i}{j}]"
                    );
                    let scale = kg[(i, i)].abs().max(kg[(j, j)].abs());
                    assert!(
                        (kg[(i, j)] - slope).abs() < 1e-6 * scale,
                        "phi {phi} kg[{i}{j}]"
                    );
                }
            }
        }
    }
}
