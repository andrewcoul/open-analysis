# Solver Plan

Plan for the calculation engine behind open-analysis, an open-source alternative to ETABS / SAP2000.

## Decision: Rust core

The engine is written in Rust. Reasons:

- **Performance without a runtime.** Native speed for element assembly, load combination loops, and iterative analyses, with no garbage collector.
- **Safe parallelism.** The ownership model lets us parallelize aggressively without data races. This directly addresses the single-threaded bottleneck common in commercial tools.
- **Portability.** One core compiles to native, to WebAssembly for a browser front end, and to Python via PyO3 for scripting and verification.
- **Numerical ecosystem.** `faer` for dense and sparse linear algebra (including multithreaded sparse Cholesky), `nalgebra` for small fixed-size matrices, `rayon` for parallelism.

Alternatives considered and rejected:

- **C++.** Deepest FEA ecosystem, but memory safety and build story are worse.
- **Julia.** Great numerics, but hard to embed in a desktop app or browser.
- **Python (NumPy/SciPy).** Fastest path to a prototype, but caps performance later.

## Reference implementation: Pynite

[Pynite](https://github.com/JWock82/Pynite) is the reference for element formulations and behaviour. It is **not** ported literally. Its object model (dictionaries of nodes, members, plates held on a `FEModel3D`) was designed for Python convenience and fights Rust's ownership model.

What we take from Pynite:

- Element formulations (3D frame, plate, quad) and coordinate transformations.
- Load types, load cases, and load combination semantics.
- Analysis procedures: linear static, P-Delta, tension-only / compression-only members.
- The test suite, ported as acceptance tests.

What we redesign:

- Data layout (see below).
- Separation of model definition from assembled system.
- Sparse-first linear algebra.

## Architecture

### Data layout

- The model is stored as **flat arrays indexed by integer IDs**, not a graph of linked objects. Nodes, members, plates, materials, sections, supports, and loads each live in their own table.
- Integer IDs replace object references everywhere. This makes parallel access trivial and keeps the WebAssembly and Python bindings simple.
- The **model definition** is immutable during analysis. The **assembled system** (stiffness matrix, DOF numbering, load vectors) is a separate structure built from the model.
- A **load combination result** is an immutable value produced by a pure function of a factored system. This is what makes parallel combos free.
- **Factorization sharing depends on the analysis type.** Linear combinations share one factorization of the elastic stiffness. P-Delta and tension-only / compression-only analyses have combination-specific stiffness and active-member state, so each such combination owns its own stiffness and may refactor during iteration. Pynite handles these states separately per combination as well. The architecture must make this distinction explicit: a factored system is keyed by its stiffness state, not assumed global.

### Crate layout

```
open-analysis/
  Cargo.toml              # workspace
  crates/
    oa-core/              # model, elements, assembly, analysis
    oa-py/                # PyO3 bindings
    oa-wasm/              # wasm-bindgen bindings (later)
  docs/
    solver/
```

### Linear algebra

- **Sparse from day one.** The global stiffness matrix is sparse CSC. Dense storage is only used for element-level matrices (12x12 frame, 24x24 plate, etc.).
- **Direct solver.** Sparse Cholesky / LDLᵀ via `faer`. Factor once per stiffness state, back-substitute once per load combination.
- **Eigenvalue solver.** See [Eigenvalue solver](#eigenvalue-solver) under Decisions.

### Parallelism

Use `rayon`. No hand-rolled thread pools.

Parallelized, in order of payoff:

1. **Load combinations.** Independent back-substitution and recovery per combo. Near-linear scaling.
2. **Design checks.** Independent per member per combo.
3. **Member force and stress recovery.** Independent per member per combo.
4. **Element stiffness computation.** Independent per element.
5. **Nonlinear iterations.** Each P-Delta or tension-only iteration reuses items 1 to 4.
6. **Sparse factorization.** Delegated to `faer`'s multithreaded Cholesky. Not written by us.

Assembly into the global matrix uses per-thread partial triplet lists merged at the end. This avoids locking on the shared matrix.

Known ceiling: sparse factorization is memory-bandwidth bound. Expect near-linear speedup on combos, recovery, and checks, and sub-linear speedup on the solve itself.

## Verification

An engine nobody trusts is not useful. Verification is a first-class deliverable.

- Port Pynite's test suite as Rust integration tests in `oa-core`.
- Add textbook benchmarks (Hibbeler, McGuire/Gallagher/Ziemian, AISC design examples) with hand-calculated expected values.
- Use the PyO3 binding to run the same model through Pynite and open-analysis and diff the results during development.
- Every element formulation and analysis type gets a benchmark before it is considered done.

## Roadmap

Each phase is complete only when its tests pass.

### Phase 0: Interfaces

- Settle the unit system and typed public API (see Decisions).
- Choose the quantity-type approach (`uom` or hand-written newtypes).

### Phase 1: Foundation

- Cargo workspace with `oa-core` and `oa-py`.
- Model tables: nodes, materials, sections, 3D frame members, supports, nodal and member loads.
- DOF numbering with support for restrained DOFs.
- 3D frame element stiffness and transformation.
- Sparse assembly and direct solve via `faer`.
- Linear static analysis for a single load case.
- Node displacements and member end forces.
- Acceptance: Pynite's basic frame tests, simple hand-calc benchmarks, and the unit-equivalence benchmark.

### Phase 2: Load combinations and parallelism

- Load cases and load combinations with factors.
- Parallel combo solve and recovery via `rayon`.
- Result consumer interface (see Decisions). In-memory collector only.
- Bounded concurrency so parallel combos do not hold unbounded results in flight.
- Member internal force diagrams (shear, moment, axial, torsion along the length).
- Member releases (pinned ends).
- Acceptance: Pynite combo tests. Benchmark showing scaling with core count.

### Phase 3: Nonlinear

- P-Delta (geometric stiffness) iteration.
- Tension-only and compression-only members.
- Acceptance: Pynite P-Delta and TC tests.

### Phase 4: Plates and shells

- Rectangular plate and quad elements.
- Surface loads.
- Acceptance: Pynite plate tests and classical plate bending benchmarks.

### Phase 5: Dynamics

- Lumped mass matrix assembly, with explicit mass versus weight density handling.
- Eigensolver prototype on `faer` Krylov-Schur, gated per Decisions.
- Modal analysis.
- Response spectrum analysis.
- Acceptance: classical eigenvalue benchmarks, eigenpair residuals, mass orthogonality, Sturm sequence count.

### Phase 6: Bindings and integration

- WebAssembly build via `oa-wasm`.
- Stable serialization format for models and results.
- Hooks for a GUI and design modules.

## Implementation status

As of 2026-09-14, a first pass of all six phases exists in `crates/`. All 25
tests pass, clippy is clean with warnings denied, and both bindings build.

| Phase | Status | Notes |
|-------|--------|-------|
| 0 Interfaces | Done | Hand-written newtypes in `units.rs`, not `uom`. `MassDensity` and `WeightDensity` are distinct; conversion requires explicit gravity. |
| 1 Foundation | Done | Frame element, sparse Cholesky via `faer`, prescribed displacements, unit-equivalence test. |
| 2 Combinations | Done | `ResultConsumer` trait with in-memory collector, `max_in_flight` bound, `Envelope` with governing combination, on-demand section forces. Pynite comparison in `scripts/benchmark_pynite.py`; results in the README and `benchmarks/`. |
| 3 Nonlinear | Done | P-Delta and tension/compression-only, per-combination stiffness state, checked against closed form and Pynite. |
| 4 Shells | Done | Rectangular Kirchhoff and DKMQ quads with membrane, surface pressure, lumped mass. |
| 5 Dynamics | Done | Lumped mass, `faer` Krylov-Schur behind an `EigenBackend` trait, Sturm count via LBLᵀ, CQC and SRSS spectrum. |
| 6 Bindings | Done | PyO3 and wasm-bindgen wrap one versioned JSON protocol. No release packaging yet. |

Deviations from the plan discovered during implementation:

- **Dense fallback in the eigensolver.** `faer` 0.24.4 panics when the Krylov
  subspace dimension reaches the operator size. Operators with at most 64
  massive DOFs, or where twice the requested mode count reaches the operator
  size, are formed densely and solved with the dense symmetric solver. This
  is bounded by `dense_limit` and never touches the global stiffness matrix.
- **Sturm pivoting is intranodal.** `faer`'s sparse LBLᵀ pivots only within
  supernodes. Nonfinite or near-zero pivots are rejected with an error rather
  than silently counted. A fully pivoted indefinite factorization is not
  available in pure Rust yet.
- **Spectrum periods must be covered.** A modal period outside the supplied
  spectrum's period range is an error, not a clamp. This is deliberate.
- **Shell drilling stiffness is artificial.** A weak rotational spring, the
  same approach as Pynite, stabilizes the drilling DOF. It is excluded from
  stress recovery and bounded by `drilling_ratio`.

Not yet done:

- A binary result path. The 2026-09-14 benchmark shows JSON serialization and
  Python parsing take a large share of API time on big models.
- Disk-backed or envelope-only result consumers.
- Rigid-body modes in modal analysis.
- Consistent mass matrices.
- Design checks.

## Decisions

These were open questions, resolved after review.

### Units

**SI internally, typed quantities at the public API.**

- The engine uses metres, newtons, kilograms, seconds, and radians throughout. Assembled matrices and vectors are plain `f64`.
- Inputs are converted once when building the model. Results are converted for display or export at the API boundary.
- Rust callers use quantity types such as `Length`, `Force`, `Pressure`. Whether these come from `uom` or a small set of hand-written newtypes is decided in Phase 0. `uom` is generic-heavy, slows compiles, and does not cross the PyO3 or WebAssembly boundary, so newtypes with explicit constructors like `Length::from_feet` are the likely choice.
- **Mass density and weight density are distinct types.** Converting weight density to mass density requires an explicit gravitational acceleration. The problem is the ambiguous imperial `lb`: lbm/ft³ is a mass density and lbf/ft³ is a weight density, and inputs frequently do not say which. Every imperial density input must declare one or the other. Guessing wrong corrupts every dynamic result and is a known class of error.
- Acceptance: a benchmark where equivalent SI and imperial inputs produce equivalent results within tolerance.
- Settled in Phase 0 because it affects every interface.

### Eigenvalue solver

**First candidate is `faer`'s matrix-free Krylov-Schur solver. ARPACK-NG is the fallback. Decided by prototype at the start of Phase 5.**

[`faer::matrix_free::eigen`](https://docs.rs/faer/latest/faer/matrix_free/eigen/index.html) provides Krylov-Schur eigensolvers including a self-adjoint variant (`partial_self_adjoint_eigen`). Verified against the docs on 2026-09-14: it handles **standard** problems only, targeting **largest-magnitude** eigenvalues. No generalized problem or shift-invert support. We write the adapter.

Adapter for the structural problem K φ = ω² M φ:

- With **lumped (diagonal) mass**, form the operator A = M^½ K⁻¹ M^½. It is symmetric, so the self-adjoint solver applies. Its largest eigenvalues μ are 1/ω² for the lowest modes, which is what we want. K⁻¹ is applied through the sparse factorization of K, never formed.
- **Requires K positive-definite,** meaning the structure is fully restrained against rigid-body motion. Either require stable supports as a precondition of modal analysis, or add a shift so that K − σM is definite. Rigid-body modes are not handled in the first implementation.
- **Physical mode-shape recovery.** The eigenvectors y of A are in transformed coordinates, and massless DOFs still have nonzero physical displacements and rotations. For each eigenpair (μ, y), recover φ = K⁻¹ M^½ y, then mass-normalize. This satisfies K φ = (1/μ) M φ exactly. Do not recover through M^-½ y, which divides by zero on massless DOFs.
- Passing K⁻¹M directly to a symmetric solver is incorrect. It is self-adjoint only in the M inner product, not the standard one.
- Consistent mass would need a Cholesky factor of M in place of M^½. Lumped mass is standard for buildings, so start there.

Gating criteria for adopting the `faer` backend:

- Frequency accuracy against classical benchmarks.
- Eigenpair residuals ‖Kφ − ω²Mφ‖ small relative to ‖Kφ‖.
- Mass orthogonality φᵢᵀ M φⱼ ≈ δᵢⱼ.
- Correct handling of repeated and closely spaced modes.
- Correct handling of massless DOFs.
- **Sturm sequence check.** Count eigenvalues below a trial frequency ω_trial using the inertia of the LDLᵀ factorization of K − σM with σ = ω_trial². The number of negative pivots equals the number of modes below ω_trial, which confirms none were missed. This is **not** free: K − σM is a different matrix from K and needs its own numerical factorization at each shift. The symbolic analysis can be reused because diagonal M does not change the sparsity pattern. The indefinite solver's inertia count must be verified, including how 2×2 pivot blocks from Bunch-Kaufman pivoting are counted. [Regularization](https://docs.rs/faer/latest/faer/linalg/cholesky/ldlt/factor/struct.LdltRegularization.html) perturbs the pivots and can change the signs being counted, so it must be disabled or accounted for explicitly during the check.

Fallback: [ARPACK-NG](https://github.com/opencollab/arpack-ng) supports generalized problems and shift-invert directly, but its Fortran and BLAS/LAPACK dependencies add packaging work and likely rule out the browser target.

The eigensolver backend sits behind a trait so it can be replaced. `faer`'s `matrix_free` module is relatively new and the crate's API is not stable, so pin the version.

### Result storage

**Start in memory. Establish an incremental output interface in Phase 2. Defer the disk backend.**

- Analysis emits results per combination, or per bounded batch, through a small result-consumer trait. The default implementation collects everything in memory.
- Callers select which combinations and which output fields they want. Detailed force-diagram samples along members are generated on demand, not stored.
- Envelopes retain the governing combination for each value. This is required for design, not optional.
- Bounded concurrency: parallel combo solves must not hold an unbounded number of results in flight.

The memory risk is predictable. Displacements alone for a large model:

| Nodes | DOFs per node | Combinations | Bytes per value | Total |
|---------|---------------|--------------|-----------------|--------|
| 100,000 | 6             | 1,000        | 8               | 4.8 GB |

Retaining every result is therefore not part of the solver's contract. A disk-backed or envelope-only consumer can be added later without changing the analysis code.

Guardrail: in Phase 2 the consumer interface is one trait with one method, and the in-memory collector is the only implementation. Do not build the disk format or a second backend until a measured model needs it.
