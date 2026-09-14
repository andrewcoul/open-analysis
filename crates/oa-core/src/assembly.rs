use crate::{
    Error, Result,
    element::{
        frame::{FrameElement, FrameState, V12},
        shell::ShellElement,
    },
    model::*,
};
use faer::{
    Mat, Side,
    prelude::Solve,
    sparse::{SparseColMat, Triplet, linalg::solvers::Llt},
};
#[cfg(feature = "parallel")]
use rayon::prelude::*;

pub(crate) struct Prepared {
    pub frames: Vec<FrameElement>,
    pub shells: Vec<ShellElement>,
    pub ndof: usize,
}
impl Prepared {
    pub fn new(model: &Model) -> Result<Self> {
        model.validate()?;
        #[cfg(feature = "parallel")]
        let frames = (0..model.frames.len())
            .into_par_iter()
            .map(|i| FrameElement::new(model, i))
            .collect::<Result<Vec<_>>>()?;
        #[cfg(not(feature = "parallel"))]
        let frames = (0..model.frames.len())
            .map(|i| FrameElement::new(model, i))
            .collect::<Result<Vec<_>>>()?;
        #[cfg(feature = "parallel")]
        let shells = (0..model.shells.len())
            .into_par_iter()
            .map(|i| ShellElement::new(model, i))
            .collect::<Result<Vec<_>>>()?;
        #[cfg(not(feature = "parallel"))]
        let shells = (0..model.shells.len())
            .map(|i| ShellElement::new(model, i))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            frames,
            shells,
            ndof: model.nodes.len() * 6,
        })
    }
    pub fn loads(&self, model: &Model, combo: &LoadCombination) -> Loads {
        let mut out = Loads {
            nodal: vec![0.0; self.ndof],
            member: vec![V12::zeros(); self.frames.len()],
            pressure: vec![0.0; self.shells.len()],
        };
        for &(id, factor) in &combo.terms {
            let case = &model.load_cases[id.0];
            for l in &case.nodal {
                for (i, v) in l.values().into_iter().enumerate() {
                    out.nodal[l.node.0 * 6 + i] += factor * v;
                }
            }
            for l in &case.member {
                out.member[l.member().0] += self.frames[l.member().0].equivalent_load(l) * factor;
            }
            for l in &case.surface {
                out.pressure[l.shell.0] += factor * l.pressure.si();
            }
        }
        out
    }
    pub fn states(
        &self,
        loads: &Loads,
        axial: &[f64],
        active: &[bool],
    ) -> Result<Vec<Option<FrameState>>> {
        self.frames
            .iter()
            .enumerate()
            .map(|(i, e)| {
                if active[i] {
                    e.state(axial[i], loads.member[i]).map(Some)
                } else {
                    Ok(None)
                }
            })
            .collect()
    }
    pub fn assemble(&self, model: &Model, states: &[Option<FrameState>]) -> Result<SparseSystem> {
        let mut entries = Vec::with_capacity(self.frames.len() * 144 + self.shells.len() * 576);
        for (e, state) in self.frames.iter().zip(states) {
            if let Some(s) = state {
                push_matrix(&mut entries, &e.dofs, &s.global_k);
            }
        }
        for e in &self.shells {
            push_matrix(&mut entries, &e.dofs, &e.global_k());
        }
        SparseSystem::new(model, entries)
    }
    pub fn force(&self, loads: &Loads, states: &[Option<FrameState>]) -> Vec<f64> {
        let mut f = loads.nodal.clone();
        for (e, state) in self.frames.iter().zip(states) {
            if let Some(s) = state {
                for i in 0..12 {
                    f[e.dofs[i]] += s.global_p[i];
                }
            }
        }
        for (i, e) in self.shells.iter().enumerate() {
            let p = e.t.transpose() * e.pressure_load * loads.pressure[i];
            for j in 0..24 {
                f[e.dofs[j]] += p[j];
            }
        }
        f
    }
    pub fn mass(&self, model: &Model) -> Vec<f64> {
        let mut mass = vec![0.0; self.ndof];
        for (i, n) in model.nodes.iter().enumerate() {
            for j in 0..3 {
                mass[6 * i + j] = n.mass[j].si();
                mass[6 * i + 3 + j] = n.mass_inertia[j].si();
            }
        }
        for e in &self.frames {
            for j in 0..3 {
                mass[e.dofs[j]] += e.mass / 2.0;
                mass[e.dofs[6 + j]] += e.mass / 2.0;
            }
        }
        for e in &self.shells {
            for i in 0..4 {
                for j in 0..3 {
                    mass[e.dofs[6 * i + j]] += e.nodal_mass[i];
                }
            }
        }
        mass
    }
}

pub(crate) struct Loads {
    pub nodal: Vec<f64>,
    pub member: Vec<V12>,
    pub pressure: Vec<f64>,
}

fn push_matrix<const N: usize>(
    out: &mut Vec<Triplet<usize, usize, f64>>,
    dofs: &[usize; N],
    k: &nalgebra::SMatrix<f64, N, N>,
) {
    for i in 0..N {
        for j in 0..N {
            let value = (k[(i, j)] + k[(j, i)]) / 2.0;
            if value != 0.0 {
                out.push(Triplet::new(dofs[i], dofs[j], value));
            }
        }
    }
}

pub(crate) struct SparseSystem {
    pub full: SparseColMat<usize, f64>,
    pub free: Vec<usize>,
    pub map: Vec<Option<usize>>,
    scale: Vec<f64>,
    factor: Option<Llt<usize, f64>>,
    prescribed: Vec<f64>,
    restrained: Vec<bool>,
}
impl SparseSystem {
    fn new(model: &Model, entries: Vec<Triplet<usize, usize, f64>>) -> Result<Self> {
        let n = model.nodes.len() * 6;
        if entries.iter().any(|v| !v.val.is_finite()) {
            return Err(Error::Model("element stiffness overflow".into()));
        }
        let full = SparseColMat::try_new_from_triplets(n, n, &entries)
            .map_err(|e| Error::Solver(format!("sparse assembly: {e:?}")))?;
        let mut diag = vec![0.0; n];
        for j in 0..n {
            for (i, &v) in full.row_idx_of_col(j).zip(full.val_of_col(j)) {
                if i == j {
                    diag[i] = v;
                }
            }
        }
        let restrained: Vec<_> = model.nodes.iter().flat_map(|n| n.restrained).collect();
        let prescribed: Vec<_> = model
            .nodes
            .iter()
            .flat_map(|n| n.prescribed.values())
            .collect();
        let mut free = vec![];
        let mut map = vec![None; n];
        let mut scale = vec![];
        for i in 0..n {
            if !restrained[i] {
                if diag[i] <= 0.0 {
                    if i % 6 < 3 || diag[i] < 0.0 {
                        return Err(Error::Unstable(format!(
                            "node {} DOF {} has no positive stiffness",
                            i / 6,
                            i % 6
                        )));
                    }
                    continue;
                }
                map[i] = Some(free.len());
                free.push(i);
                scale.push(1.0 / diag[i].sqrt());
            }
        }
        let mut reduced = vec![];
        for j in 0..n {
            if let Some(c) = map[j] {
                for (i, &v) in full.row_idx_of_col(j).zip(full.val_of_col(j)) {
                    if let Some(r) = map[i] {
                        reduced.push(Triplet::new(r, c, v * scale[r] * scale[c]));
                    }
                }
            }
        }
        let factor = if free.is_empty() {
            None
        } else {
            let k = SparseColMat::try_new_from_triplets(free.len(), free.len(), &reduced)
                .map_err(|e| Error::Solver(format!("reduced stiffness: {e:?}")))?;
            let factor = k.sp_cholesky(Side::Lower).map_err(|e| {
                Error::Unstable(format!(
                    "Cholesky failed ({e:?}); check restraints, releases, connectivity, or buckling"
                ))
            })?;
            // Detect nearly singular mechanisms that survive roundoff in Cholesky.
            // Diagonal equilibration makes this probe independent of uniform stiffness scaling.
            let probe = Mat::from_fn(free.len(), 1, |i, _| {
                ((i as f64 + 1.0) * 1.61803398875).sin() + 0.37
            });
            let inverse_probe = factor.solve(&probe);
            if (0..free.len())
                .any(|i| !inverse_probe[(i, 0)].is_finite() || inverse_probe[(i, 0)].abs() > 1e12)
            {
                return Err(Error::Unstable(
                    "near-singular equilibrated stiffness (mechanism or extreme conditioning)"
                        .into(),
                ));
            }
            Some(factor)
        };
        Ok(Self {
            full,
            free,
            map,
            scale,
            factor,
            prescribed,
            restrained,
        })
    }
    pub fn apply(&self, u: &[f64]) -> Vec<f64> {
        sparse_apply(&self.full, u)
    }
    pub fn solve_free(&self, rhs: &[f64]) -> Vec<f64> {
        let Some(factor) = &self.factor else {
            return vec![];
        };
        let b = Mat::from_fn(self.free.len(), 1, |i, _| rhs[i] * self.scale[i]);
        let x = factor.solve(&b);
        (0..self.free.len())
            .map(|i| x[(i, 0)] * self.scale[i])
            .collect()
    }
    pub fn solve(&self, f: &[f64]) -> Result<(Vec<f64>, Vec<f64>, f64)> {
        if f.iter().any(|x| !x.is_finite()) {
            return Err(Error::Model("load accumulation overflow".into()));
        }
        for (i, &v) in f.iter().enumerate() {
            if !self.restrained[i] && self.map[i].is_none() && v.abs() > 1e-12 {
                return Err(Error::Unstable(format!(
                    "load on inactive node {} DOF {}",
                    i / 6,
                    i % 6
                )));
            }
        }
        let known = self.apply(&self.prescribed);
        let rhs: Vec<_> = self.free.iter().map(|&i| f[i] - known[i]).collect();
        let mut x = self.solve_free(&rhs);
        let mut u = self.prescribed.clone();
        for (i, &dof) in self.free.iter().enumerate() {
            u[dof] = x[i];
        }
        // One iterative-refinement step also improves reaction recovery on stiff frames.
        let ku = self.apply(&u);
        let correction =
            self.solve_free(&self.free.iter().map(|&i| f[i] - ku[i]).collect::<Vec<_>>());
        for i in 0..x.len() {
            x[i] += correction[i];
            u[self.free[i]] = x[i];
        }
        if u.iter().any(|x| !x.is_finite()) {
            return Err(Error::Solver("nonfinite displacements".into()));
        }
        let ku = self.apply(&u);
        let residual = self
            .free
            .iter()
            .map(|&i| (ku[i] - f[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        let norm = self
            .free
            .iter()
            .map(|&i| f[i].powi(2) + known[i].powi(2))
            .sum::<f64>()
            .sqrt()
            .max(1.0);
        let rel = residual / norm;
        if rel > 1e-7 {
            return Err(Error::Solver(format!(
                "equilibrium residual {rel:e} exceeds 1e-7"
            )));
        }
        let reactions = (0..u.len())
            .map(|i| {
                if self.restrained[i] {
                    ku[i] - f[i]
                } else {
                    0.0
                }
            })
            .collect();
        Ok((u, reactions, rel))
    }
}
pub(crate) fn sparse_apply(k: &SparseColMat<usize, f64>, u: &[f64]) -> Vec<f64> {
    let mut f = vec![0.0; k.nrows()];
    for (j, &d) in u.iter().enumerate() {
        for (i, &v) in k.row_idx_of_col(j).zip(k.val_of_col(j)) {
            f[i] += v * d;
        }
    }
    f
}
