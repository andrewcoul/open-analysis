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
    pub springs: Vec<f64>,
    pub constraints: Vec<Option<Vec<(usize, f64)>>>,
    masters: Vec<bool>,
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
            springs: model.nodes.iter().flat_map(Node::springs).collect(),
            constraints: model.constraints(),
            masters: model.diaphragm_masters(),
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
            if case.self_weight != [0.0; 3] {
                let g = model.gravity.si();
                for (i, e) in self.frames.iter().enumerate() {
                    if let Some(load) = e.self_weight_load(FrameId(i), g, case.self_weight) {
                        out.member[i] += e.equivalent_load(&load) * factor;
                    }
                }
                for e in &self.shells {
                    for (corner, &m) in e.nodal_mass.iter().enumerate() {
                        for (axis, &w) in case.self_weight.iter().enumerate() {
                            out.nodal[e.dofs[6 * corner + axis]] += factor * m * g * w;
                        }
                    }
                }
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
        SparseSystem::new(model, entries, self)
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

/// Calls `f(master_dof, coefficient)` for each independent DOF that physical DOF
/// `i` depends on: itself when independent, its masters when a slave, nothing
/// when restrained. Together these define u = T q + u_prescribed.
fn for_each_term(
    constraints: &[Option<Vec<(usize, f64)>>],
    restrained: &[bool],
    i: usize,
    mut f: impl FnMut(usize, f64),
) {
    if restrained[i] {
        return;
    }
    match &constraints[i] {
        Some(terms) => {
            for &(d, c) in terms {
                f(d, c);
            }
        }
        None => f(i, 1.0),
    }
}

/// Factored reduced stiffness Tᵀ K T over the independent active DOFs.
pub(crate) struct SparseSystem {
    /// Full-space stiffness including grounded springs.
    pub full: SparseColMat<usize, f64>,
    /// Reduced index -> physical DOF of each independent active DOF.
    pub free: Vec<usize>,
    /// Physical DOF -> reduced index for independent active DOFs.
    pub map: Vec<Option<usize>>,
    constraints: Vec<Option<Vec<(usize, f64)>>>,
    /// Unscaled reduced stiffness entries, both triangles present.
    reduced: Vec<Triplet<usize, usize, f64>>,
    scale: Vec<f64>,
    factor: Option<Llt<usize, f64>>,
    /// Full-space displacement implied by prescribed values, slaves included.
    prescribed: Vec<f64>,
    restrained: Vec<bool>,
    springs: Vec<f64>,
}
impl SparseSystem {
    fn new(
        model: &Model,
        mut entries: Vec<Triplet<usize, usize, f64>>,
        prep: &Prepared,
    ) -> Result<Self> {
        let n = model.nodes.len() * 6;
        for (i, &k) in prep.springs.iter().enumerate() {
            if k > 0.0 {
                entries.push(Triplet::new(i, i, k));
            }
        }
        if entries.iter().any(|v| !v.val.is_finite()) {
            return Err(Error::Model("element stiffness overflow".into()));
        }
        let full = SparseColMat::try_new_from_triplets(n, n, &entries)
            .map_err(|e| Error::Solver(format!("sparse assembly: {e:?}")))?;
        let restrained: Vec<_> = model.nodes.iter().flat_map(|n| n.restrained).collect();
        let constraints = prep.constraints.clone();
        let mut provisional = vec![None; n];
        let mut count = 0;
        for i in 0..n {
            if !restrained[i] && constraints[i].is_none() {
                provisional[i] = Some(count);
                count += 1;
            }
        }
        let mut diag = vec![0.0; count];
        let mut reduced = Vec::with_capacity(entries.len());
        for t in &entries {
            for_each_term(&constraints, &restrained, t.row, |a, ca| {
                for_each_term(&constraints, &restrained, t.col, |b, cb| {
                    if let (Some(ra), Some(rb)) = (provisional[a], provisional[b]) {
                        let v = ca * cb * t.val;
                        if v != 0.0 {
                            if ra == rb {
                                diag[ra] += v;
                            }
                            reduced.push(Triplet::new(ra, rb, v));
                        }
                    }
                });
            });
        }
        let mut map = vec![None; n];
        let mut free = vec![];
        let mut keep = vec![None; count];
        for i in 0..n {
            let Some(p) = provisional[i] else {
                continue;
            };
            if diag[p] <= 0.0 {
                let droppable = i % 6 >= 3 || prep.masters[i / 6];
                if diag[p] < 0.0 || !droppable {
                    return Err(Error::Unstable(format!(
                        "node {} DOF {} has no positive stiffness",
                        i / 6,
                        i % 6
                    )));
                }
                continue;
            }
            keep[p] = Some(free.len());
            map[i] = Some(free.len());
            free.push(i);
        }
        let scale: Vec<f64> = free
            .iter()
            .map(|&i| 1.0 / diag[provisional[i].unwrap()].sqrt())
            .collect();
        let reduced: Vec<_> = reduced
            .into_iter()
            .filter_map(|t| Some(Triplet::new(keep[t.row]?, keep[t.col]?, t.val)))
            .collect();
        let factor = if free.is_empty() {
            None
        } else {
            let scaled: Vec<_> = reduced
                .iter()
                .map(|t| Triplet::new(t.row, t.col, t.val * scale[t.row] * scale[t.col]))
                .collect();
            let k = SparseColMat::try_new_from_triplets(free.len(), free.len(), &scaled)
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
        let mut system = Self {
            full,
            free,
            map,
            constraints,
            reduced,
            scale,
            factor,
            prescribed: vec![],
            restrained,
            springs: prep.springs.clone(),
        };
        let raw: Vec<f64> = model
            .nodes
            .iter()
            .flat_map(|n| n.prescribed.values())
            .collect();
        system.prescribed = system.expand_with(&vec![0.0; system.free.len()], &raw);
        Ok(system)
    }
    pub fn is_restrained(&self, dof: usize) -> bool {
        self.restrained[dof]
    }
    /// Independent-and-kept or slave of a kept master; false for restrained or dropped DOFs.
    pub fn is_active(&self, dof: usize) -> bool {
        self.map[dof].is_some() || self.constraints[dof].is_some()
    }
    pub fn constraint(&self, dof: usize) -> Option<&[(usize, f64)]> {
        self.constraints[dof].as_deref()
    }
    pub fn reduced_entries(&self) -> &[Triplet<usize, usize, f64>] {
        &self.reduced
    }
    pub fn scale(&self) -> &[f64] {
        &self.scale
    }
    fn for_each_term(&self, i: usize, f: impl FnMut(usize, f64)) {
        for_each_term(&self.constraints, &self.restrained, i, f);
    }
    /// Tᵀ v: project a full-space vector onto the independent active DOFs.
    pub fn reduce(&self, v: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.free.len()];
        for (i, &value) in v.iter().enumerate() {
            if value == 0.0 {
                continue;
            }
            self.for_each_term(i, |d, c| {
                if let Some(r) = self.map[d] {
                    out[r] += c * value;
                }
            });
        }
        out
    }
    /// Full-space displacements: prescribed at restraints, zero at dropped DOFs,
    /// reduced unknowns at independent DOFs, and master-derived values at slaves.
    pub fn expand(&self, x: &[f64]) -> Vec<f64> {
        self.expand_with(x, &self.prescribed)
    }
    fn expand_with(&self, x: &[f64], prescribed: &[f64]) -> Vec<f64> {
        let n = self.map.len();
        let mut u = vec![0.0; n];
        for i in 0..n {
            if self.restrained[i] {
                u[i] = prescribed[i];
            } else if let Some(r) = self.map[i] {
                u[i] = x[r];
            }
        }
        for i in 0..n {
            if let Some(terms) = &self.constraints[i] {
                u[i] = terms.iter().map(|&(d, c)| c * u[d]).sum();
            }
        }
        u
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
            if !self.restrained[i] && !self.is_active(i) && v.abs() > 1e-12 {
                return Err(Error::Unstable(format!(
                    "load on inactive node {} DOF {}",
                    i / 6,
                    i % 6
                )));
            }
        }
        let known = self.apply(&self.prescribed);
        let rhs: Vec<f64> = f.iter().zip(&known).map(|(a, b)| a - b).collect();
        let mut x = self.solve_free(&self.reduce(&rhs));
        // One iterative-refinement step also improves reaction recovery on stiff frames.
        let ku = self.apply(&self.expand(&x));
        let rhs: Vec<f64> = f.iter().zip(&ku).map(|(a, b)| a - b).collect();
        for (xi, c) in x.iter_mut().zip(self.solve_free(&self.reduce(&rhs))) {
            *xi += c;
        }
        let u = self.expand(&x);
        if u.iter().any(|x| !x.is_finite()) {
            return Err(Error::Solver("nonfinite displacements".into()));
        }
        let ku = self.apply(&u);
        let out_of_balance: Vec<f64> = ku.iter().zip(f).map(|(a, b)| a - b).collect();
        let residual = norm(&self.reduce(&out_of_balance));
        let norm = norm(&self.reduce(f))
            .hypot(norm(&self.reduce(&known)))
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
                } else if self.springs[i] > 0.0 {
                    -self.springs[i] * u[i]
                } else {
                    0.0
                }
            })
            .collect();
        Ok((u, reactions, rel))
    }
}
pub(crate) fn norm(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
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
