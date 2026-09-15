#[cfg(feature = "parallel")]
use crate::exec::{ASSEMBLY_CHUNK, PARALLEL_THRESHOLD};
use crate::{
    Error, Result,
    element::{
        frame::{FrameElement, FrameState, FrameStiffness, Stiffness, V12},
        shell::{M24, ShellElement},
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
    /// Zero-axial-force stiffness of every frame, shared by every linear
    /// combination and by nonlinear members whose axial force is zero.
    pub elastic: Vec<FrameStiffness>,
    /// Global stiffness of every shell, cached once per preparation.
    pub shell_k: Vec<M24>,
    pub ndof: usize,
    pub springs: Vec<f64>,
    pub constraints: Vec<Option<Vec<(usize, f64)>>>,
    masters: Vec<bool>,
    /// Equivalent loads of each load case, combined by factor per combination.
    case_loads: Vec<Loads>,
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
        #[cfg(feature = "parallel")]
        let elastic = frames.par_iter().map(|e| e.stiffness(0.0)).collect();
        #[cfg(not(feature = "parallel"))]
        let elastic = frames.iter().map(|e| e.stiffness(0.0)).collect();
        #[cfg(feature = "parallel")]
        let shell_k = shells.par_iter().map(ShellElement::global_k).collect();
        #[cfg(not(feature = "parallel"))]
        let shell_k = shells.iter().map(ShellElement::global_k).collect();
        let mut prep = Self {
            frames,
            shells,
            elastic,
            shell_k,
            ndof: model.nodes.len() * 6,
            springs: model.nodes.iter().flat_map(Node::springs).collect(),
            constraints: model.constraints(),
            masters: model.diaphragm_masters(),
            case_loads: vec![],
        };
        prep.case_loads = model
            .load_cases
            .iter()
            .map(|case| prep.case_loads(model, case))
            .collect();
        Ok(prep)
    }
    fn case_loads(&self, model: &Model, case: &LoadCase) -> Loads {
        let mut out = self.zero_loads();
        for l in &case.nodal {
            for (i, v) in l.values().into_iter().enumerate() {
                out.nodal[l.node.0 * 6 + i] += v;
            }
        }
        for l in &case.member {
            out.member[l.member().0] += self.frames[l.member().0].equivalent_load(l);
        }
        for l in &case.surface {
            out.pressure[l.shell.0] += l.pressure.si();
        }
        if case.self_weight != [0.0; 3] {
            let g = model.gravity.si();
            for (i, e) in self.frames.iter().enumerate() {
                if let Some(load) = e.self_weight_load(FrameId(i), g, case.self_weight) {
                    out.member[i] += e.equivalent_load(&load);
                }
            }
            for e in &self.shells {
                for (corner, &m) in e.nodal_mass.iter().enumerate() {
                    for (axis, &w) in case.self_weight.iter().enumerate() {
                        out.nodal[e.dofs[6 * corner + axis]] += m * g * w;
                    }
                }
            }
        }
        out
    }
    pub fn zero_loads(&self) -> Loads {
        Loads {
            nodal: vec![0.0; self.ndof],
            member: vec![V12::zeros(); self.frames.len()],
            pressure: vec![0.0; self.shells.len()],
        }
    }
    pub fn loads(&self, combo: &LoadCombination) -> Loads {
        let mut out = self.zero_loads();
        for &(id, factor) in &combo.terms {
            let case = &self.case_loads[id.0];
            for (o, v) in out.nodal.iter_mut().zip(&case.nodal) {
                *o += factor * v;
            }
            for (o, v) in out.member.iter_mut().zip(&case.member) {
                *o += v * factor;
            }
            for (o, v) in out.pressure.iter_mut().zip(&case.pressure) {
                *o += factor * v;
            }
        }
        out
    }
    /// Frame states for the given axial forces and active set. Members at zero
    /// axial force borrow the cached elastic stiffness; others condense afresh.
    pub fn states(
        &self,
        loads: &Loads,
        axial: &[f64],
        active: &[bool],
    ) -> Result<Vec<Option<FrameState<'_>>>> {
        let one = |i: usize| -> Result<Option<FrameState<'_>>> {
            if !active[i] {
                return Ok(None);
            }
            let e = &self.frames[i];
            let stiffness = if axial[i] == 0.0 {
                Stiffness::Shared(&self.elastic[i])
            } else {
                Stiffness::Owned(Box::new(e.stiffness(axial[i])))
            };
            e.state(stiffness, loads.member[i]).map(Some)
        };
        // Only a nonzero axial force condenses a fresh stiffness; borrowing the
        // cached one is too cheap to justify nested parallelism inside a batch
        // of concurrent combinations.
        #[cfg(feature = "parallel")]
        if self.frames.len() >= PARALLEL_THRESHOLD && axial.iter().any(|&n| n != 0.0) {
            return (0..self.frames.len()).into_par_iter().map(one).collect();
        }
        (0..self.frames.len()).map(one).collect()
    }
    /// Unloaded, fully active, zero-axial states: the shared linear stiffness.
    pub fn elastic_states(&self) -> Result<Vec<Option<FrameState<'_>>>> {
        self.states(
            &self.zero_loads(),
            &vec![0.0; self.frames.len()],
            &vec![true; self.frames.len()],
        )
    }
    /// Element triplets in fixed element order. Chunks of elements fill
    /// private buffers in parallel and are concatenated in order, so the
    /// summation order inside the sparse matrix does not depend on scheduling.
    pub fn assemble(&self, model: &Model, states: &[Option<FrameState>]) -> Result<SparseSystem> {
        let frame_chunk = |pairs: &[(&FrameElement, &Option<FrameState>)]| {
            let mut out = Vec::with_capacity(pairs.len() * 144);
            for (e, state) in pairs {
                if let Some(s) = state {
                    push_matrix(&mut out, &e.dofs, s.global_k());
                }
            }
            out
        };
        let shell_chunk = |pairs: &[(&ShellElement, &M24)]| {
            let mut out = Vec::with_capacity(pairs.len() * 576);
            for (e, k) in pairs {
                push_matrix(&mut out, &e.dofs, k);
            }
            out
        };
        let pairs: Vec<_> = self.frames.iter().zip(states).collect();
        let shells: Vec<_> = self.shells.iter().zip(&self.shell_k).collect();
        #[cfg(feature = "parallel")]
        let buffers: Vec<Vec<Triplet<usize, usize, f64>>> =
            if self.frames.len() + self.shells.len() >= PARALLEL_THRESHOLD {
                pairs
                    .par_chunks(ASSEMBLY_CHUNK)
                    .map(frame_chunk)
                    .chain(shells.par_chunks(ASSEMBLY_CHUNK).map(shell_chunk))
                    .collect()
            } else {
                vec![frame_chunk(&pairs), shell_chunk(&shells)]
            };
        #[cfg(not(feature = "parallel"))]
        let buffers = vec![frame_chunk(&pairs), shell_chunk(&shells)];
        let mut entries =
            Vec::with_capacity(buffers.iter().map(Vec::len).sum::<usize>() + self.ndof);
        for buffer in buffers {
            entries.extend(buffer);
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
    /// Row-sum norm of the reduced stiffness, for backward-error scaling.
    norm_inf: f64,
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
        let mut row_sums = vec![0.0; free.len()];
        for t in &reduced {
            row_sums[t.row] += t.val.abs();
        }
        let norm_inf = row_sums.iter().copied().fold(0.0, f64::max);
        let mut system = Self {
            full,
            free,
            map,
            constraints,
            reduced,
            norm_inf,
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
        let load_scale = norm(&self.reduce(f)).hypot(norm(&self.reduce(&known)));
        // Iterative refinement recovers digits lost in the factorization of
        // badly conditioned systems. Each step gains roughly a factor of
        // kappa * epsilon, so a few steps suffice whenever the factor is usable.
        let mut u = self.expand(&x);
        let mut ku = self.apply(&u);
        let mut rel = f64::INFINITY;
        for _ in 0..=MAX_REFINEMENT_STEPS {
            let out_of_balance: Vec<f64> = ku.iter().zip(f).map(|(a, b)| a - b).collect();
            let reduced = self.reduce(&out_of_balance);
            // Backward error: the residual relative to the size of the terms
            // that produced it, ||K|| ||x|| + ||f||. A residual near that floor
            // means the solution is as accurate as the conditioning allows, and
            // no refinement can lower it further. Relative to ||f|| alone, a
            // badly conditioned but perfectly well solved system would be
            // rejected for large displacements it genuinely has.
            let x_norm = x.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
            let scale = (self.norm_inf * x_norm + load_scale).max(1.0);
            let next = norm(&reduced) / scale;
            if !next.is_finite() {
                return Err(Error::Solver("nonfinite displacements".into()));
            }
            let stalled = next > rel * 0.5;
            rel = next.min(rel);
            if next <= RESIDUAL_TOLERANCE || stalled {
                break;
            }
            for (xi, c) in x.iter_mut().zip(self.solve_free(&reduced)) {
                *xi -= c;
            }
            u = self.expand(&x);
            ku = self.apply(&u);
        }
        if rel > RESIDUAL_TOLERANCE {
            return Err(Error::Solver(format!(
                "equilibrium residual {rel:e} exceeds {RESIDUAL_TOLERANCE:e} after refinement; the stiffness matrix is too badly conditioned"
            )));
        }
        let out_of_balance: Vec<f64> = ku.iter().zip(f).map(|(a, b)| a - b).collect();
        let reactions = self.reactions(&out_of_balance, &u);
        Ok((u, reactions, rel))
    }
    /// Support and spring reactions from the full-space out-of-balance vector
    /// g = K u - f. A restrained DOF carries its own row of g plus the
    /// constraint force of every slave DOF that depends on it, which is the
    /// restrained part of Tᵀ g. Without that transfer a restrained diaphragm
    /// master would report no reaction for loads applied at its slaves.
    pub fn reactions(&self, g: &[f64], u: &[f64]) -> Vec<f64> {
        let mut out: Vec<f64> = (0..u.len())
            .map(|i| {
                if self.restrained[i] {
                    g[i]
                } else if self.springs[i] > 0.0 {
                    -self.springs[i] * u[i]
                } else {
                    0.0
                }
            })
            .collect();
        for (slave, terms) in self.constraints.iter().enumerate() {
            let Some(terms) = terms else {
                continue;
            };
            if self.restrained[slave] {
                continue;
            }
            for &(master, c) in terms {
                if self.restrained[master] {
                    out[master] += c * g[slave];
                }
            }
        }
        out
    }
}
/// Relative equilibrium residual accepted from a linear solve.
pub const RESIDUAL_TOLERANCE: f64 = 1e-7;
/// Refinement steps after the first solve before giving up.
pub const MAX_REFINEMENT_STEPS: usize = 8;
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
