use crate::{
    AxialBehavior, Error, Model, Result,
    analysis::node_values,
    assembly::{Prepared, SparseSystem, norm},
    exec::Exec,
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
use nalgebra::{DMatrix, DVector};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// Worker count for the whole run. Zero uses the current Rayon pool.
    /// Ignored in a serial/WASM build.
    pub threads: usize,
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
            threads: 0,
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
pub trait EigenBackend: Sync {
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

/// Factor L of the reduced mass Tᵀ M T on its massive subspace, so M_r = L Lᵀ.
/// Independent DOFs carry sqrt(m). Each diaphragm master carries a dense block
/// that couples in-plane translation and normal rotation through slave offsets,
/// so the master need not sit at the centre of mass.
struct MassRoot {
    scalar: Vec<(usize, f64)>,
    blocks: Vec<MassBlock>,
    dim: usize,
}
struct MassBlock {
    dofs: Vec<usize>,
    matrix: DMatrix<f64>,
    /// dofs x rank, matrix = factor * factorᵀ.
    factor: DMatrix<f64>,
}
impl MassRoot {
    fn new(system: &SparseSystem, mass: &[f64]) -> Result<Self> {
        let mut diag = vec![0.0; system.free.len()];
        let mut coupled: BTreeMap<usize, BTreeMap<(usize, usize), f64>> = BTreeMap::new();
        for (i, &m) in mass.iter().enumerate() {
            if m <= 0.0 || system.is_restrained(i) {
                continue;
            }
            match system.constraint(i) {
                None => {
                    if let Some(r) = system.map[i] {
                        diag[r] += m;
                    }
                }
                Some(terms) => {
                    let block = coupled.entry(terms[0].0 / 6).or_default();
                    for &(a, ca) in terms {
                        for &(b, cb) in terms {
                            if let (Some(ra), Some(rb)) = (system.map[a], system.map[b]) {
                                *block.entry((ra, rb)).or_default() += ca * cb * m;
                            }
                        }
                    }
                }
            }
        }
        let mut blocks = vec![];
        let mut dim = 0;
        for entries in coupled.into_values() {
            let mut dofs: Vec<usize> = entries.keys().flat_map(|&(a, b)| [a, b]).collect();
            dofs.sort_unstable();
            dofs.dedup();
            if dofs.is_empty() {
                continue;
            }
            let k = dofs.len();
            let mut matrix = DMatrix::zeros(k, k);
            for (&(a, b), &v) in &entries {
                let ia = dofs.binary_search(&a).unwrap();
                let ib = dofs.binary_search(&b).unwrap();
                matrix[(ia, ib)] += v;
            }
            for (ia, &d) in dofs.iter().enumerate() {
                matrix[(ia, ia)] += diag[d];
                diag[d] = 0.0;
            }
            let eig = matrix.clone().symmetric_eigen();
            let max: f64 = eig.eigenvalues.amax();
            if eig.eigenvalues.iter().any(|&v| v < -1e-12 * max) {
                return Err(Error::Solver(
                    "reduced diaphragm mass is not positive semidefinite".into(),
                ));
            }
            let kept: Vec<usize> = (0..k)
                .filter(|&j| eig.eigenvalues[j] > 1e-12 * max)
                .collect();
            let factor = DMatrix::from_fn(k, kept.len(), |i, j| {
                let root: f64 = eig.eigenvalues[kept[j]];
                eig.eigenvectors[(i, kept[j])] * root.sqrt()
            });
            dim += kept.len();
            blocks.push(MassBlock {
                dofs,
                matrix,
                factor,
            });
        }
        let scalar: Vec<_> = diag
            .iter()
            .enumerate()
            .filter(|(_, m)| **m > 0.0)
            .map(|(r, m)| (r, m.sqrt()))
            .collect();
        dim += scalar.len();
        Ok(Self {
            scalar,
            blocks,
            dim,
        })
    }
    /// out += L y, out in reduced space.
    fn apply_l(&self, y: &[f64], out: &mut [f64]) {
        let mut j = 0;
        for &(r, s) in &self.scalar {
            out[r] += s * y[j];
            j += 1;
        }
        for b in &self.blocks {
            let k = b.factor.ncols();
            let v = &b.factor * DVector::from_column_slice(&y[j..j + k]);
            for (i, &d) in b.dofs.iter().enumerate() {
                out[d] += v[i];
            }
            j += k;
        }
    }
    /// out = Lᵀ v, out of length dim.
    fn apply_lt(&self, v: &[f64], out: &mut [f64]) {
        let mut j = 0;
        for &(r, s) in &self.scalar {
            out[j] = s * v[r];
            j += 1;
        }
        for b in &self.blocks {
            let y = b.factor.transpose() * DVector::from_fn(b.dofs.len(), |i, _| v[b.dofs[i]]);
            out[j..j + y.len()].copy_from_slice(y.as_slice());
            j += y.len();
        }
    }
    /// Reduced-space mass entries, both triangles.
    fn entries(&self) -> Vec<Triplet<usize, usize, f64>> {
        let mut out: Vec<_> = self
            .scalar
            .iter()
            .map(|&(r, s)| Triplet::new(r, r, s * s))
            .collect();
        for b in &self.blocks {
            for (i, &a) in b.dofs.iter().enumerate() {
                for (j, &c) in b.dofs.iter().enumerate() {
                    if b.matrix[(i, j)] != 0.0 {
                        out.push(Triplet::new(a, c, b.matrix[(i, j)]));
                    }
                }
            }
        }
        out
    }
}

/// Lᵀ K_r⁻¹ L: symmetric, and its largest eigenvalues are 1/ω² of the lowest modes.
struct MassOperator<'a> {
    system: &'a SparseSystem,
    root: &'a MassRoot,
}
impl std::fmt::Debug for MassOperator<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MassOperator")
            .field("dimension", &self.root.dim)
            .finish()
    }
}
impl LinOp<f64> for MassOperator<'_> {
    fn nrows(&self) -> usize {
        self.root.dim
    }
    fn ncols(&self) -> usize {
        self.root.dim
    }
    fn apply_scratch(&self, _: usize, _: Par) -> StackReq {
        StackReq::EMPTY
    }
    fn apply(&self, mut out: MatMut<'_, f64>, rhs: MatRef<'_, f64>, _: Par, _: &mut MemStack) {
        for col in 0..rhs.ncols() {
            let y: Vec<f64> = (0..self.root.dim).map(|i| rhs[(i, col)]).collect();
            let mut f = vec![0.0; self.system.free.len()];
            self.root.apply_l(&y, &mut f);
            let x = self.system.solve_free(&f);
            let mut result = vec![0.0; self.root.dim];
            self.root.apply_lt(&x, &mut result);
            for (i, v) in result.into_iter().enumerate() {
                out[(i, col)] = v;
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
    validate_options(options)?;
    let exec = Exec::new(options.threads)?;
    exec.install(|| {
        let prep = Prepared::new(model)?;
        analyze_modal_prepared(model, &prep, options, backend)
    })
}
fn validate_options(options: &ModalOptions) -> Result<()> {
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
    Ok(())
}
/// Modal analysis on an existing preparation, so spectrum analysis shares it.
/// Runs in the caller's pool.
pub(crate) fn analyze_modal_prepared(
    model: &Model,
    prep: &Prepared,
    options: &ModalOptions,
    backend: &dyn EigenBackend,
) -> Result<ModalResult> {
    validate_options(options)?;
    let (system, mass) = modal_system(model, prep)?;
    let root = MassRoot::new(&system, &mass)?;
    if root.dim == 0 {
        return Err(Error::Request(
            "modal analysis needs positive mass on free DOFs".into(),
        ));
    }
    let operator = MassOperator {
        system: &system,
        root: &root,
    };
    let count = options.modes.min(root.dim);
    let (values, vectors) = backend.solve(&operator, count, options)?;
    if values.len() != count || vectors.nrows() != root.dim || vectors.ncols() != count {
        return Err(Error::Solver(
            "eigen backend returned invalid dimensions".into(),
        ));
    }
    let mut order: Vec<_> = (0..count).collect();
    order.sort_by(|&a, &b| values[b].total_cmp(&values[a]));
    let total_free_mass = std::array::from_fn(|axis| {
        (0..prep.ndof)
            .filter(|&d| d % 6 == axis && !system.is_restrained(d) && system.is_active(d))
            .map(|d| mass[d])
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
        // Physical shape: phi = T K_r^-1 L y, so massless and slave DOFs are recovered.
        let y: Vec<f64> = (0..root.dim).map(|i| vectors[(i, j)]).collect();
        let mut f = vec![0.0; system.free.len()];
        root.apply_l(&y, &mut f);
        let mut shape = system.expand(&system.solve_free(&f));
        let norm_m = shape
            .iter()
            .zip(&mass)
            .map(|(v, m)| m * v * v)
            .sum::<f64>()
            .sqrt();
        if !norm_m.is_finite() || norm_m <= 0.0 {
            return Err(Error::Solver("invalid mode mass normalization".into()));
        }
        for v in &mut shape {
            *v /= norm_m;
        }
        // Deterministic sign: largest mass-weighted component is positive.
        let peak = (0..prep.ndof)
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
        let out_of_balance: Vec<f64> = (0..prep.ndof)
            .map(|d| kphi[d] - lambda * mass[d] * shape[d])
            .collect();
        let residual = norm(&system.reduce(&out_of_balance));
        let knorm = norm(&system.reduce(&kphi));
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
            (0..prep.ndof)
                .filter(|&d| d % 6 == a)
                .map(|d| mass[d] * shape[d])
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
        let counter = SturmCounter::new(&system, &root)?;
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
    let prep = Prepared::new(model)?;
    let (system, mass) = modal_system(model, &prep)?;
    let root = MassRoot::new(&system, &mass)?;
    SturmCounter::new(&system, &root)?.count((std::f64::consts::TAU * frequency_hz).powi(2))
}
fn modal_system(model: &Model, prep: &Prepared) -> Result<(SparseSystem, Vec<f64>)> {
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
    let system = prep.assemble(model, &prep.elastic_states()?)?;
    let mass = prep.mass(model);
    for (d, &m) in mass.iter().enumerate() {
        if m > 0.0 && !system.is_restrained(d) && !system.is_active(d) {
            return Err(Error::Unstable(format!(
                "mass on unstiffened node {} DOF {}",
                d / 6,
                d % 6
            )));
        }
    }
    Ok((system, mass))
}

/// Inertia of K_r - σ M_r via sparse LBLᵀ. Each shift needs a numeric
/// factorization; only the symbolic analysis is reused.
struct SturmCounter {
    stiffness: Vec<Triplet<usize, usize, f64>>,
    mass: Vec<Triplet<usize, usize, f64>>,
    symbolic: cholesky::SymbolicCholesky<usize>,
}
impl SturmCounter {
    fn new(system: &SparseSystem, root: &MassRoot) -> Result<Self> {
        let n = system.free.len();
        let scale = system.scale();
        let stiffness: Vec<_> = system
            .reduced_entries()
            .iter()
            .map(|t| Triplet::new(t.row, t.col, t.val * scale[t.row] * scale[t.col]))
            .collect();
        let mass: Vec<_> = root
            .entries()
            .into_iter()
            .map(|t| Triplet::new(t.row, t.col, t.val * scale[t.row] * scale[t.col]))
            .collect();
        // Symbolic pattern is the union of both, so every shift shares it.
        let pattern = Self::shifted(&stiffness, &mass, 0.0, n)?;
        let params = cholesky::CholeskySymbolicParams {
            supernodal_flop_ratio_threshold: SupernodalThreshold::FORCE_SUPERNODAL,
            ..Default::default()
        };
        let symbolic = cholesky::factorize_symbolic_cholesky(
            pattern.symbolic(),
            Side::Lower,
            Default::default(),
            params,
        )
        .map_err(|e| Error::Solver(format!("Sturm symbolic analysis: {e:?}")))?;
        Ok(Self {
            stiffness,
            mass,
            symbolic,
        })
    }
    fn shifted(
        stiffness: &[Triplet<usize, usize, f64>],
        mass: &[Triplet<usize, usize, f64>],
        sigma: f64,
        n: usize,
    ) -> Result<SparseColMat<usize, f64>> {
        let entries: Vec<_> = stiffness
            .iter()
            .copied()
            .chain(
                mass.iter()
                    .map(|t| Triplet::new(t.row, t.col, -sigma * t.val)),
            )
            .collect();
        SparseColMat::try_new_from_triplets(n, n, &entries)
            .map_err(|e| Error::Solver(format!("Sturm shifted matrix: {e:?}")))
    }
    fn count(&self, sigma: f64) -> Result<usize> {
        let n = self.symbolic.nrows();
        let k = Self::shifted(&self.stiffness, &self.mass, sigma, n)?;
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
