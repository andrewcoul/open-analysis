//! Four-node flat shells: bilinear plane stress plus DKMQ or rectangular
//! 12-term Kirchhoff bending. DKMQ equations follow Pynite's Quad3D and
//! Katili et al.; attribution and reference revision are in THIRD_PARTY.md.
use super::{GAUSS3, block_rotation, rotation};
use crate::{Error, Result, model::*};
use nalgebra::{Matrix2, Matrix3, SMatrix, SVector, Vector2, Vector3};

pub(crate) type M24 = SMatrix<f64, 24, 24>;
pub(crate) type V24 = SVector<f64, 24>;
type B3 = SMatrix<f64, 3, 24>;
type B2 = SMatrix<f64, 2, 24>;

#[derive(Clone)]
pub(crate) struct ShellElement {
    pub dofs: [usize; 24],
    pub t: M24,
    pub k: M24,
    pub pressure_load: V24,
    pub nodal_mass: [f64; 4],
    /// Center stress, moment and shear recovery operators applied to local displacements.
    stress_op: B3,
    moment_op: B3,
    shear_op: B2,
    xy: [Vector2<f64>; 4],
    thickness: f64,
    poisson: f64,
    rectangle_inverse: Option<SMatrix<f64, 12, 12>>,
}

impl ShellElement {
    pub fn new(model: &Model, id: usize) -> Result<Self> {
        let shell = &model.shells[id];
        let material = &model.materials[shell.material.0];
        let p = shell.nodes.map(|n| Vector3::from(model.nodes[n.0].xyz()));
        let edge = p[1] - p[0];
        let len = edge.norm();
        let normal = edge.cross(&(p[3] - p[0]));
        if len <= 1e-12 || normal.norm() <= 1e-12 * len * len {
            return Err(Error::Model(format!("shell {id}: degenerate corners")));
        }
        let x = edge / len;
        let z = normal.normalize();
        let y = z.cross(&x);
        let r = rotation(x, y);
        let xy = p.map(|v| {
            let q = r * (v - p[0]);
            Vector2::new(q.x, q.y)
        });
        let scale = p.iter().map(|v| (v - p[0]).norm()).fold(0.0_f64, f64::max);
        if p.iter().any(|v| z.dot(&(v - p[0])).abs() > 1e-8 * scale) {
            return Err(Error::Model(format!(
                "shell {id}: warped corners; subdivide into coplanar elements"
            )));
        }
        for i in 0..4 {
            let e = xy[(i + 1) % 4] - xy[i];
            let f = xy[(i + 2) % 4] - xy[(i + 1) % 4];
            if e.x * f.y - e.y * f.x <= 1e-12 * scale * scale {
                return Err(Error::Model(format!(
                    "shell {id}: corners must form a convex counterclockwise quadrilateral"
                )));
            }
        }
        if shell.formulation == ShellFormulation::Rectangular
            && (xy[2].x - xy[1].x)
                .abs()
                .max(xy[3].x.abs())
                .max((xy[2].y - xy[3].y).abs())
                > 1e-8 * scale
        {
            return Err(Error::Model(format!(
                "shell {id}: rectangular formulation requires a rectangle"
            )));
        }
        let e = material.young.si();
        let nu = material.poisson;
        let h = shell.thickness.si();
        let dm = Matrix3::new(1.0, nu, 0.0, nu, 1.0, 0.0, 0.0, 0.0, (1.0 - nu) / 2.0)
            * (e / (1.0 - nu * nu));
        let db = dm * (h.powi(3) / 12.0);
        let ds = Matrix2::identity() * (5.0 / 6.0 * material.shear_modulus() * h);
        let mut out = Self {
            dofs: std::array::from_fn(|i| shell.nodes[i / 6].0 * 6 + i % 6),
            t: block_rotation(&r),
            k: M24::zeros(),
            pressure_load: V24::zeros(),
            nodal_mass: [0.0; 4],
            stress_op: B3::zeros(),
            moment_op: B3::zeros(),
            shear_op: B2::zeros(),
            xy,
            thickness: h,
            poisson: nu,
            rectangle_inverse: None,
        };
        if shell.formulation == ShellFormulation::Rectangular {
            let mut c = SMatrix::<f64, 12, 12>::zeros();
            for (n, (a, b)) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .into_iter()
                .enumerate()
            {
                c.row_mut(3 * n).copy_from(&poly(a, b, 0, 0).transpose());
                c.row_mut(3 * n + 1)
                    .copy_from(&(poly(a, b, 0, 1) * (2.0 / xy[3].y)).transpose());
                c.row_mut(3 * n + 2)
                    .copy_from(&(poly(a, b, 1, 0) * (-2.0 / xy[1].x)).transpose());
            }
            out.rectangle_inverse = Some(c.try_inverse().ok_or_else(|| {
                Error::Model(format!("shell {id}: singular rectangular interpolation"))
            })?);
        }
        // Bending: 3x3 integrates the rectangular polynomial exactly; DKMQ uses 2x2.
        let gps: Vec<(f64, f64)> = if shell.formulation == ShellFormulation::Rectangular {
            GAUSS3.to_vec()
        } else {
            let g = 1.0 / 3.0_f64.sqrt();
            vec![(-g, 1.0), (g, 1.0)]
        };
        for &(a, wa) in &gps {
            for &(b, wb) in &gps {
                let (_, _, det) = out.geometry(a, b)?;
                let w = wa * wb * det;
                let (bb, bs) = out.bending(a, b)?;
                out.k += (bb.transpose() * db * bb + bs.transpose() * ds * bs) * w;
                let shape = out.transverse_shape(a, b);
                out.pressure_load += shape * w;
            }
        }
        // Membrane and diagonal translational mass.
        let g = 1.0 / 3.0_f64.sqrt();
        for a in [-g, g] {
            for b in [-g, g] {
                let (n, _, det) = out.geometry(a, b)?;
                let bm = out.membrane(a, b)?;
                out.k += bm.transpose() * dm * bm * (h * det);
                for (mass, shape) in out.nodal_mass.iter_mut().zip(n) {
                    *mass += material.density.si() * h * shape * det;
                }
            }
        }
        // Same weak drilling stabilization used by the Pynite reference.
        // This artificial stiffness is reported/documented and excluded from stress recovery.
        let kd = (0..4)
            .flat_map(|i| {
                [
                    out.k[(6 * i + 3, 6 * i + 3)].abs(),
                    out.k[(6 * i + 4, 6 * i + 4)].abs(),
                ]
            })
            .fold(f64::INFINITY, f64::min)
            * shell.drilling_ratio;
        for i in 0..4 {
            out.k[(6 * i + 5, 6 * i + 5)] = kd;
        }
        let (bb, bs) = out.bending(0.0, 0.0)?;
        out.stress_op = dm * out.membrane(0.0, 0.0)?;
        out.moment_op = db * bb;
        out.shear_op = ds * bs;
        Ok(out)
    }
    fn geometry(&self, xi: f64, eta: f64) -> Result<([f64; 4], [Vector2<f64>; 4], f64)> {
        let n = [
            (1.0 - xi) * (1.0 - eta) / 4.0,
            (1.0 + xi) * (1.0 - eta) / 4.0,
            (1.0 + xi) * (1.0 + eta) / 4.0,
            (1.0 - xi) * (1.0 + eta) / 4.0,
        ];
        let dr = [
            (eta - 1.0) / 4.0,
            (1.0 - eta) / 4.0,
            (1.0 + eta) / 4.0,
            (-1.0 - eta) / 4.0,
        ];
        let ds = [
            (xi - 1.0) / 4.0,
            (-1.0 - xi) / 4.0,
            (1.0 + xi) / 4.0,
            (1.0 - xi) / 4.0,
        ];
        let j = self.jacobian(dr, ds);
        let det = j.determinant();
        if !det.is_finite() || det <= 0.0 {
            return Err(Error::Model("shell has nonpositive Jacobian".into()));
        }
        let inv = j
            .try_inverse()
            .ok_or_else(|| Error::Model("singular shell Jacobian".into()))?;
        Ok((
            n,
            std::array::from_fn(|i| inv * Vector2::new(dr[i], ds[i])),
            det,
        ))
    }
    fn jacobian(&self, dr: [f64; 4], ds: [f64; 4]) -> Matrix2<f64> {
        Matrix2::new(
            (0..4).map(|i| dr[i] * self.xy[i].x).sum(),
            (0..4).map(|i| dr[i] * self.xy[i].y).sum(),
            (0..4).map(|i| ds[i] * self.xy[i].x).sum(),
            (0..4).map(|i| ds[i] * self.xy[i].y).sum(),
        )
    }
    fn membrane(&self, xi: f64, eta: f64) -> Result<B3> {
        let (_, grad, _) = self.geometry(xi, eta)?;
        let mut b = B3::zeros();
        for i in 0..4 {
            b[(0, 6 * i)] = grad[i].x;
            b[(1, 6 * i + 1)] = grad[i].y;
            b[(2, 6 * i)] = grad[i].y;
            b[(2, 6 * i + 1)] = grad[i].x;
        }
        Ok(b)
    }
    fn bending(&self, xi: f64, eta: f64) -> Result<(B3, B2)> {
        if let Some(inv) = &self.rectangle_inverse {
            let mut curvature = SMatrix::<f64, 3, 12>::zeros();
            let a = self.xy[1].x / 2.0;
            let b = self.xy[3].y / 2.0;
            curvature
                .row_mut(0)
                .copy_from(&(poly(xi, eta, 2, 0) * (-1.0 / (a * a))).transpose());
            curvature
                .row_mut(1)
                .copy_from(&(poly(xi, eta, 0, 2) * (-1.0 / (b * b))).transpose());
            curvature
                .row_mut(2)
                .copy_from(&(poly(xi, eta, 1, 1) * (-2.0 / (a * b))).transpose());
            let small = curvature * inv;
            let mut bb = B3::zeros();
            for i in 0..4 {
                for j in 0..3 {
                    for k in 0..3 {
                        bb[(k, 6 * i + 2 + j)] = small[(k, 3 * i + j)];
                    }
                }
            }
            return Ok((bb, B2::zeros()));
        }
        let (_, grad, _) = self.geometry(xi, eta)?;
        let dr = [
            (eta - 1.0) / 4.0,
            (1.0 - eta) / 4.0,
            (1.0 + eta) / 4.0,
            (-1.0 - eta) / 4.0,
        ];
        let ds = [
            (xi - 1.0) / 4.0,
            (-1.0 - xi) / 4.0,
            (1.0 + xi) / 4.0,
            (1.0 - xi) / 4.0,
        ];
        let inv = self
            .jacobian(dr, ds)
            .try_inverse()
            .ok_or_else(|| Error::Model("singular shell Jacobian".into()))?;
        let mut beta = SMatrix::<f64, 3, 12>::zeros();
        for i in 0..4 {
            beta[(0, 3 * i + 1)] = grad[i].x;
            beta[(1, 3 * i + 2)] = grad[i].y;
            beta[(2, 3 * i + 1)] = grad[i].y;
            beta[(2, 3 * i + 2)] = grad[i].x;
        }
        let dp_r = [
            xi * (eta - 1.0),
            -0.5 * (eta - 1.0) * (eta + 1.0),
            -xi * (eta + 1.0),
            0.5 * (eta - 1.0) * (eta + 1.0),
        ];
        let dp_s = [
            0.5 * (xi - 1.0) * (xi + 1.0),
            -eta * (xi + 1.0),
            -0.5 * (xi - 1.0) * (xi + 1.0),
            eta * (xi - 1.0),
        ];
        let mut delta = SMatrix::<f64, 3, 4>::zeros();
        let mut au = SMatrix::<f64, 4, 12>::zeros();
        let mut ad = SMatrix::<f64, 4, 4>::zeros();
        let mut ag_phi = SMatrix::<f64, 4, 4>::zeros();
        for i in 0..4 {
            let j = (i + 1) % 4;
            let edge = self.xy[j] - self.xy[i];
            let l = edge.norm();
            let c = edge.x / l;
            let s = edge.y / l;
            let phi = 2.0 / ((5.0 / 6.0) * (1.0 - self.poisson)) * (self.thickness / l).powi(2);
            let gradp = inv * Vector2::new(dp_r[i], dp_s[i]);
            delta[(0, i)] = gradp.x * c;
            delta[(1, i)] = gradp.y * s;
            delta[(2, i)] = gradp.y * c + gradp.x * s;
            au[(i, 3 * i)] = -1.0 / l;
            au[(i, 3 * j)] = 1.0 / l;
            for node in [i, j] {
                au[(i, 3 * node + 1)] = c / 2.0;
                au[(i, 3 * node + 2)] = s / 2.0;
            }
            ad[(i, i)] = -1.5 / (1.0 + phi);
            ag_phi[(i, i)] = (if i < 2 { 1.0 } else { -1.0 }) * l / 2.0 * phi / (1.0 + phi);
        }
        let ng = SMatrix::<f64, 2, 4>::from_row_slice(&[
            (1.0 - eta) / 2.0,
            0.0,
            (1.0 + eta) / 2.0,
            0.0,
            0.0,
            (1.0 + xi) / 2.0,
            0.0,
            (1.0 - xi) / 2.0,
        ]);
        let small_b = beta + delta * ad * au;
        let small_s = inv * ng * ag_phi * au;
        let mut bb = B3::zeros();
        let mut bs = B2::zeros();
        // DKMQ variables [w, beta_x, beta_y] = [uz, ry, -rx].
        for i in 0..4 {
            for (j, dof, sign) in [(0, 2, 1.0), (1, 4, 1.0), (2, 3, -1.0)] {
                for k in 0..3 {
                    bb[(k, 6 * i + dof)] = small_b[(k, 3 * i + j)] * sign;
                }
                for k in 0..2 {
                    bs[(k, 6 * i + dof)] = small_s[(k, 3 * i + j)] * sign;
                }
            }
        }
        Ok((bb, bs))
    }
    fn transverse_shape(&self, xi: f64, eta: f64) -> V24 {
        let mut n = V24::zeros();
        if let Some(inv) = &self.rectangle_inverse {
            let row = poly(xi, eta, 0, 0).transpose() * inv;
            for i in 0..4 {
                for j in 0..3 {
                    n[6 * i + 2 + j] = row[3 * i + j];
                }
            }
        } else {
            let ns = [
                (1.0 - xi) * (1.0 - eta) / 4.0,
                (1.0 + xi) * (1.0 - eta) / 4.0,
                (1.0 + xi) * (1.0 + eta) / 4.0,
                (1.0 - xi) * (1.0 + eta) / 4.0,
            ];
            for i in 0..4 {
                n[6 * i + 2] = ns[i];
            }
        }
        n
    }
    /// Tᵀ k T. Constant per element; `Prepared` caches it for assembly and
    /// equilibrium checks rather than the element, which keeps the element
    /// small while parallel construction moves it by value.
    pub fn global_k(&self) -> M24 {
        self.t.transpose() * self.k * self.t
    }
    pub fn recover(&self, global_d: &[f64], pressure: f64) -> Result<crate::results::ShellResult> {
        let d = self.t * V24::from_fn(|i, _| global_d[self.dofs[i]]);
        let f = self.k * d - self.pressure_load * pressure;
        let stress = self.stress_op * d;
        let moment = self.moment_op * d;
        let shear = self.shear_op * d;
        Ok(crate::results::ShellResult {
            local_end_forces: std::array::from_fn(|i| f[i]),
            membrane_stress: stress.into(),
            bending_moment: moment.into(),
            transverse_shear: shear.into(),
        })
    }
}

fn poly(x: f64, y: f64, dx: u32, dy: u32) -> SVector<f64, 12> {
    let powers: [(u32, u32); 12] = [
        (0, 0),
        (1, 0),
        (0, 1),
        (2, 0),
        (1, 1),
        (0, 2),
        (3, 0),
        (2, 1),
        (1, 2),
        (0, 3),
        (3, 1),
        (1, 3),
    ];
    SVector::from_fn(|i, _| {
        let (a, b) = powers[i];
        if a < dx || b < dy {
            0.0
        } else {
            (0..dx).map(|k| (a - k) as f64).product::<f64>()
                * (0..dy).map(|k| (b - k) as f64).product::<f64>()
                * x.powi((a - dx) as i32)
                * y.powi((b - dy) as i32)
        }
    })
}
