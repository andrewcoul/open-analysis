use crate::{
    AxialBehavior, Error, Model, Result,
    analysis::node_values,
    assembly::{Loads, Prepared, SparseSystem},
};
use faer::{
    Col, Mat, MatMut, MatRef, Par, Side,
    dyn_stack::{MemBuffer, MemStack, StackReq},
    matrix_free::{LinOp, eigen},
    sparse::{
        SparseColMat, Triplet,
        linalg::{SupernodalThreshold, cholesky},
    },
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModalOptions {
    pub modes: usize,
    pub tolerance: f64,
    pub max_restarts: usize,
    pub subspace_dimension: usize,
    /// Bound for the small-operator dense eigensolve (never the global K).
    pub dense_limit: usize,
    pub check_sturm: bool,
}
impl Default for ModalOptions {
    fn default() -> Self {
        Self {
            modes: 6,
            tolerance: 1e-9,
            max_restarts: 1000,
            subspace_dimension: 0,
            dense_limit: 256,
            check_sturm: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mode {
    pub eigenvalue: f64,
    pub frequency_hz: f64,
    pub period_seconds: f64,
    pub shape: Vec<[f64; 6]>,
    pub relative_residual: f64,
    pub participation: [f64; 3],
    pub effective_mass: [f64; 3],
    pub mass_ratio: [f64; 3],
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModalResult {
    pub backend: String,
    pub modes: Vec<Mode>,
    pub total_free_mass: [f64; 3],
    pub cumulative_mass_ratio: [f64; 3],
    pub maximum_mass_orthogonality_error: f64,
    pub sturm_count_below_cutoff: Option<usize>,
    pub sturm_cutoff_hz: Option<f64>,
}

/// Backend contract: return largest eigenpairs of a real symmetric operator.
/// Eigenvectors are columns. Implementations must report nonconvergence.
pub trait EigenBackend {
    fn name(&self) -> &str;
    fn solve(
        &self,
        operator: &dyn LinOp<f64>,
        count: usize,
        options: &ModalOptions,
    ) -> Result<(Vec<f64>, Mat<f64>)>;
}
pub struct FaerKrylovSchur;
impl EigenBackend for FaerKrylovSchur {
    fn name(&self) -> &str {
        "faer 0.24.4 Krylov-Schur / bounded dense operator"
    }
    fn solve(
        &self,
        operator: &dyn LinOp<f64>,
        count: usize,
        options: &ModalOptions,
    ) -> Result<(Vec<f64>, Mat<f64>)> {
        let n = operator.nrows();
        // Upstream's max_dim == n branch is an explicit panic in 0.24.4.
        // Compute small/full spectra using faer's dense symmetric solver instead.
        // Large global stiffness remains sparse and is only accessed via solves.
        if n <= 64 || count.saturating_mul(2) >= n || options.subspace_dimension >= n {
            if n > options.dense_limit {
                return Err(Error::Request(format!(
                    "full modal projection has {n} massive DOFs, exceeding dense_limit {}; request fewer modes / smaller subspace or explicitly increase dense_limit",
                    options.dense_limit
                )));
            }
            let mut a = Mat::zeros(n, n);
            let mut unit = Mat::zeros(n, 1);
            let mut memory = MemBuffer::new(operator.apply_scratch(1, Par::Seq));
            for j in 0..n {
                unit.fill(0.0);
                unit[(j, 0)] = 1.0;
                operator.apply(
                    a.as_mut().get_mut(.., j..j + 1),
                    unit.as_ref(),
                    Par::Seq,
                    MemStack::new(&mut memory),
                );
            }
            let eig = a
                .self_adjoint_eigen(Side::Lower)
                .map_err(|e| Error::Solver(format!("small modal eigensolve: {e:?}")))?;
            let values = (0..count).map(|i| eig.S()[n - 1 - i]).collect();
            let vectors = Mat::from_fn(n, count, |i, j| eig.U()[(i, n - 1 - j)]);
            return Ok((values, vectors));
        }
        let mut values = vec![0.0; count];
        let mut vectors = Mat::zeros(operator.nrows(), count);
        let initial = Col::from_fn(operator.nrows(), |i| {
            ((i as f64 + 1.0) * 1.61803398875).sin() + 0.173
        });
        let params = eigen::PartialEigenParams {
            max_dim: options.subspace_dimension,
            max_restarts: options.max_restarts,
            ..Default::default()
        };
        let scratch = eigen::partial_self_adjoint_eigen_scratch(operator, count, Par::Seq, params);
        let mut memory = MemBuffer::try_new(scratch)
            .map_err(|e| Error::Solver(format!("eigensolver workspace: {e:?}")))?;
        let info = eigen::partial_self_adjoint_eigen(
            vectors.as_mut(),
            &mut values,
            operator,
            initial.as_ref(),
            options.tolerance,
            Par::Seq,
            MemStack::new(&mut memory),
            params,
        );
        if info.n_converged_eigen < count {
            return Err(Error::Solver(format!(
                "only {} of {count} eigenpairs converged",
                info.n_converged_eigen
            )));
        }
        Ok((values, vectors))
    }
}

struct MassOperator<'a> {
    system: &'a SparseSystem,
    massive: Vec<usize>,
    sqrt_mass: Vec<f64>,
}
impl std::fmt::Debug for MassOperator<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MassOperator")
            .field("dimension", &self.massive.len())
            .finish()
    }
}
impl LinOp<f64> for MassOperator<'_> {
    fn nrows(&self) -> usize {
        self.massive.len()
    }
    fn ncols(&self) -> usize {
        self.massive.len()
    }
    fn apply_scratch(&self, _: usize, _: Par) -> StackReq {
        StackReq::EMPTY
    }
    fn apply(&self, mut out: MatMut<'_, f64>, rhs: MatRef<'_, f64>, _: Par, _: &mut MemStack) {
        for col in 0..rhs.ncols() {
            let mut f = vec![0.0; self.system.free.len()];
            for (i, &dof) in self.massive.iter().enumerate() {
                f[dof] = rhs[(i, col)] * self.sqrt_mass[i];
            }
            let x = self.system.solve_free(&f);
            for (i, &dof) in self.massive.iter().enumerate() {
                out[(i, col)] = x[dof] * self.sqrt_mass[i];
            }
        }
    }
    fn conj_apply(
        &self,
        out: MatMut<'_, f64>,
        rhs: MatRef<'_, f64>,
        par: Par,
        stack: &mut MemStack,
    ) {
        self.apply(out, rhs, par, stack);
    }
}

pub fn analyze_modal(model: &Model, options: &ModalOptions) -> Result<ModalResult> {
    analyze_modal_with_backend(model, options, &FaerKrylovSchur)
}
pub fn analyze_modal_with_backend(
    model: &Model,
    options: &ModalOptions,
    backend: &dyn EigenBackend,
) -> Result<ModalResult> {
    if options.modes == 0
        || options.max_restarts == 0
        || !options.tolerance.is_finite()
        || options.tolerance <= 0.0
        || options.tolerance >= 1.0
    {
        return Err(Error::Request(
            "invalid modal mode count or convergence settings".into(),
        ));
    }
    let (prep, system, mass) = modal_system(model)?;
    let massive: Vec<_> = system
        .free
        .iter()
        .enumerate()
        .filter_map(|(i, &d)| if mass[d] > 0.0 { Some(i) } else { None })
        .collect();
    if massive.is_empty() {
        return Err(Error::Request(
            "modal analysis needs positive mass on free DOFs".into(),
        ));
    }
    let sqrt_mass = massive
        .iter()
        .map(|&i| mass[system.free[i]].sqrt())
        .collect();
    let operator = MassOperator {
        system: &system,
        massive,
        sqrt_mass,
    };
    let count = options.modes.min(operator.nrows());
    let (values, vectors) = backend.solve(&operator, count, options)?;
    if values.len() != count || vectors.nrows() != operator.nrows() || vectors.ncols() != count {
        return Err(Error::Solver(
            "eigen backend returned invalid dimensions".into(),
        ));
    }
    let mut order: Vec<_> = (0..count).collect();
    order.sort_by(|&a, &b| values[b].total_cmp(&values[a]));
    let total_free_mass = std::array::from_fn(|axis| {
        system
            .free
            .iter()
            .filter(|&&d| d % 6 == axis)
            .map(|&d| mass[d])
            .sum::<f64>()
    });
    let mut modes = vec![];
    let mut full_shapes: Vec<Vec<f64>> = vec![];
    let mut max_orth = 0.0_f64;
    for &j in &order {
        let mu = values[j];
        if !mu.is_finite() || mu <= 0.0 {
            return Err(Error::Solver(
                "eigensolver returned nonpositive inverse eigenvalue".into(),
            ));
        }
        let mut f = vec![0.0; system.free.len()];
        for (i, &d) in operator.massive.iter().enumerate() {
            f[d] = operator.sqrt_mass[i] * vectors[(i, j)];
        }
        let x = system.solve_free(&f);
        let mut shape = vec![0.0; prep.ndof];
        for (i, &d) in system.free.iter().enumerate() {
            shape[d] = x[i];
        }
        let norm = shape
            .iter()
            .zip(&mass)
            .map(|(v, m)| m * v * v)
            .sum::<f64>()
            .sqrt();
        if !norm.is_finite() || norm <= 0.0 {
            return Err(Error::Solver("invalid mode mass normalization".into()));
        }
        for v in &mut shape {
            *v /= norm;
        }
        // Deterministic sign: largest mass-weighted component is positive.
        let peak = system
            .free
            .iter()
            .copied()
            .max_by(|&a, &b| {
                (shape[a].abs() * mass[a].sqrt()).total_cmp(&(shape[b].abs() * mass[b].sqrt()))
            })
            .unwrap();
        if shape[peak] < 0.0 {
            for v in &mut shape {
                *v = -*v;
            }
        }
        let lambda = 1.0 / mu;
        let kphi = system.apply(&shape);
        let residual = system
            .free
            .iter()
            .map(|&d| (kphi[d] - lambda * mass[d] * shape[d]).powi(2))
            .sum::<f64>()
            .sqrt();
        let knorm = system
            .free
            .iter()
            .map(|&d| kphi[d].powi(2))
            .sum::<f64>()
            .sqrt();
        let relative = residual / knorm.max(f64::MIN_POSITIVE);
        if !relative.is_finite() || relative > options.tolerance.max(1e-10) * 100.0 {
            return Err(Error::Solver(format!(
                "mode {} residual {relative:e} fails validation",
                modes.len() + 1
            )));
        }
        for previous in &full_shapes {
            let dot = previous
                .iter()
                .zip(&shape)
                .zip(&mass)
                .map(|((a, b), m)| a * b * m)
                .sum::<f64>();
            max_orth = max_orth.max(dot.abs());
        }
        let participation: [f64; 3] = std::array::from_fn(|a| {
            system
                .free
                .iter()
                .filter(|&&d| d % 6 == a)
                .map(|&d| mass[d] * shape[d])
                .sum()
        });
        let effective_mass = participation.map(|v| v * v);
        let mass_ratio = std::array::from_fn(|a| {
            if total_free_mass[a] > 0.0 {
                effective_mass[a] / total_free_mass[a]
            } else {
                0.0
            }
        });
        let frequency_hz = lambda.sqrt() / std::f64::consts::TAU;
        modes.push(Mode {
            eigenvalue: lambda,
            frequency_hz,
            period_seconds: 1.0 / frequency_hz,
            shape: node_values(&shape),
            relative_residual: relative,
            participation,
            effective_mass,
            mass_ratio,
        });
        full_shapes.push(shape);
    }
    if max_orth > options.tolerance.max(1e-10) * 100.0 {
        return Err(Error::Solver(format!(
            "mode mass orthogonality error {max_orth:e}"
        )));
    }
    let (sturm_count, sturm_cutoff) = if options.check_sturm {
        let counter = SturmCounter::new(&system, &mass)?;
        let top = modes.last().unwrap().eigenvalue;
        // Count below the upper mode, excluding its potentially truncated repeated
        // eigenvalue cluster. This certifies that no LOWER modes were skipped.
        let cutoff = top * (1.0 - 1e-6);
        let expected = modes.iter().filter(|m| m.eigenvalue < cutoff).count();
        let actual = counter.count(cutoff)?;
        if actual != expected {
            return Err(Error::Solver(format!(
                "Sturm check found {actual} modes below the cutoff; returned spectrum has {expected}"
            )));
        }
        (Some(actual), Some(cutoff.sqrt() / std::f64::consts::TAU))
    } else {
        (None, None)
    };
    let cumulative_mass_ratio =
        std::array::from_fn(|i| modes.iter().map(|m| m.mass_ratio[i]).sum());
    Ok(ModalResult {
        backend: backend.name().into(),
        modes,
        total_free_mass,
        cumulative_mass_ratio,
        maximum_mass_orthogonality_error: max_orth,
        sturm_count_below_cutoff: sturm_count,
        sturm_cutoff_hz: sturm_cutoff,
    })
}

pub fn modal_inertia_count(model: &Model, frequency_hz: f64) -> Result<usize> {
    if !frequency_hz.is_finite() || frequency_hz < 0.0 {
        return Err(Error::Request(
            "trial frequency must be finite and nonnegative".into(),
        ));
    }
    let (_, system, mass) = modal_system(model)?;
    SturmCounter::new(&system, &mass)?.count((std::f64::consts::TAU * frequency_hz).powi(2))
}
fn modal_system(model: &Model) -> Result<(Prepared, SparseSystem, Vec<f64>)> {
    let prep = Prepared::new(model)?;
    if model
        .frames
        .iter()
        .any(|f| f.behavior != AxialBehavior::Both)
    {
        return Err(Error::Request(
            "modal analysis requires bilateral members; provide the desired active linear model"
                .into(),
        ));
    }
    if model
        .nodes
        .iter()
        .any(|n| n.prescribed.values().iter().any(|&v| v != 0.0))
    {
        return Err(Error::Request(
            "modal analysis uses an unstressed model with zero prescribed displacements".into(),
        ));
    }
    let loads = Loads {
        nodal: vec![0.0; prep.ndof],
        member: vec![Default::default(); prep.frames.len()],
        pressure: vec![0.0; prep.shells.len()],
    };
    let states = prep.states(
        &loads,
        &vec![0.0; prep.frames.len()],
        &vec![true; prep.frames.len()],
    )?;
    let system = prep.assemble(model, &states)?;
    let mass = prep.mass(model);
    for (d, &m) in mass.iter().enumerate() {
        if m > 0.0 && !model.nodes[d / 6].restrained[d % 6] && system.map[d].is_none() {
            return Err(Error::Unstable(format!(
                "mass on unstiffened node {} DOF {}",
                d / 6,
                d % 6
            )));
        }
    }
    Ok((prep, system, mass))
}

struct SturmCounter {
    entries: Vec<Triplet<usize, usize, f64>>,
    mass: Vec<f64>,
    symbolic: cholesky::SymbolicCholesky<usize>,
}
impl SturmCounter {
    fn new(system: &SparseSystem, mass: &[f64]) -> Result<Self> {
        let n = system.free.len();
        let mut scale = vec![0.0; n];
        for (j, &d) in system.free.iter().enumerate() {
            for (i, &v) in system.full.row_idx_of_col(d).zip(system.full.val_of_col(d)) {
                if i == d {
                    scale[j] = 1.0 / v.sqrt();
                }
            }
        }
        let mut entries = vec![];
        for (j, &d) in system.free.iter().enumerate() {
            for (i, &v) in system.full.row_idx_of_col(d).zip(system.full.val_of_col(d)) {
                if let Some(r) = system.map[i] {
                    entries.push(Triplet::new(r, j, v * scale[r] * scale[j]));
                }
            }
        }
        let k = SparseColMat::try_new_from_triplets(n, n, &entries)
            .map_err(|e| Error::Solver(format!("Sturm assembly: {e:?}")))?;
        let params = cholesky::CholeskySymbolicParams {
            supernodal_flop_ratio_threshold: SupernodalThreshold::FORCE_SUPERNODAL,
            ..Default::default()
        };
        let symbolic = cholesky::factorize_symbolic_cholesky(
            k.symbolic(),
            Side::Lower,
            Default::default(),
            params,
        )
        .map_err(|e| Error::Solver(format!("Sturm symbolic analysis: {e:?}")))?;
        let mass = system
            .free
            .iter()
            .enumerate()
            .map(|(i, &d)| mass[d] * scale[i] * scale[i])
            .collect();
        Ok(Self {
            entries,
            mass,
            symbolic,
        })
    }
    fn count(&self, sigma: f64) -> Result<usize> {
        let n = self.mass.len();
        let mut entries = self.entries.clone();
        for e in &mut entries {
            if e.row == e.col {
                e.val -= sigma * self.mass[e.row];
            }
        }
        let k = SparseColMat::try_new_from_triplets(n, n, &entries)
            .map_err(|e| Error::Solver(format!("Sturm shifted matrix: {e:?}")))?;
        let mut values = vec![0.0; self.symbolic.len_val()];
        let mut subdiag = vec![0.0; n];
        let mut p = vec![0; n];
        let mut pi = vec![0; n];
        let scratch = self
            .symbolic
            .factorize_numeric_intranode_lblt_scratch::<f64>(Par::Seq, Default::default());
        let mut memory = MemBuffer::try_new(scratch)
            .map_err(|e| Error::Solver(format!("Sturm workspace: {e:?}")))?;
        // faer 0.24.4's LBLT API performs intranodal Bunch-Kaufman pivoting
        // without dynamic regularization. Reject singular/uncertain factors.
        let _factor = self.symbolic.factorize_numeric_intranode_lblt(
            &mut values,
            &mut subdiag,
            &mut p,
            &mut pi,
            k.as_ref(),
            Side::Lower,
            Par::Seq,
            MemStack::new(&mut memory),
            Default::default(),
        );
        if values.iter().chain(&subdiag).any(|v| !v.is_finite()) {
            return Err(Error::Solver(
                "Sturm factorization is singular; move the trial frequency away from an eigenvalue"
                    .into(),
            ));
        }
        let cholesky::SymbolicCholeskyRaw::Supernodal(symbolic) = self.symbolic.raw() else {
            return Err(Error::Solver("Sturm requires supernodal pivoting".into()));
        };
        let factor = cholesky::supernodal::SupernodalLdltRef::new(symbolic, &values);
        let mut diagonal = vec![0.0; n];
        for s in 0..symbolic.n_supernodes() {
            let node = factor.supernode(s);
            for j in 0..node.val().ncols() {
                diagonal[node.start() + j] = node.val()[(j, j)];
            }
        }
        count_blocks(&diagonal, &subdiag)
    }
}

fn count_blocks(diag: &[f64], sub: &[f64]) -> Result<usize> {
    let mut count = 0;
    let mut i = 0;
    while i < diag.len() {
        if sub[i] != 0.0 {
            if i + 1 >= diag.len() {
                return Err(Error::Solver("invalid last 2x2 inertia pivot".into()));
            }
            let a = diag[i];
            let b = sub[i];
            let c = diag[i + 1];
            let scale = a.abs().max(b.abs()).max(c.abs());
            let (a, b, c) = (a / scale, b / scale, c / scale);
            let det = a * c - b * b;
            if det.abs() < 1e-14 {
                return Err(Error::Solver(
                    "Sturm count is ambiguous near a zero 2x2 pivot".into(),
                ));
            }
            count += if det < 0.0 {
                1
            } else if a + c < 0.0 {
                2
            } else {
                0
            };
            i += 2;
        } else {
            if diag[i].abs() < 1e-14 {
                return Err(Error::Solver(
                    "Sturm count is ambiguous near a zero pivot".into(),
                ));
            }
            if diag[i] < 0.0 {
                count += 1;
            }
            i += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inertia_counts_two_by_two_blocks() {
        assert_eq!(count_blocks(&[0.0, 0.0, 3.0], &[2.0, 0.0, 0.0]).unwrap(), 1);
        assert_eq!(count_blocks(&[-3.0, -3.0], &[1.0, 0.0]).unwrap(), 2);
        assert!(count_blocks(&[1.0, 1.0], &[1.0, 0.0]).is_err());
    }
}
