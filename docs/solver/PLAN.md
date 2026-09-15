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

As of 2026-09-14, a first pass of all six phases plus the building features
in Phase 7 exists in `crates/`. All 32 tests pass, clippy is clean with
warnings denied, and both bindings build. The editable model layer that sits
above the solver is planned separately in [docs/model/PLAN.md](../model/PLAN.md).

| Phase | Status | Notes |
|-------|--------|-------|
| 0 Interfaces | Done | Hand-written newtypes in `units.rs`, not `uom`. `MassDensity` and `WeightDensity` are distinct; conversion requires explicit gravity. |
| 1 Foundation | Done | Frame element, sparse Cholesky via `faer`, prescribed displacements, unit-equivalence test. |
| 2 Combinations | Done | `ResultConsumer` trait with in-memory collector, `max_in_flight` bound, `Envelope` with governing combination, on-demand section forces. Pynite comparison in `scripts/benchmark_pynite.py`; results in the README and `benchmarks/`. |
| 3 Nonlinear | Done | P-Delta and tension/compression-only, per-combination stiffness state, checked against closed form and Pynite. |
| 4 Shells | Done | Rectangular Kirchhoff and DKMQ quads with membrane, surface pressure, lumped mass. |
| 5 Dynamics | Done | Lumped mass, `faer` Krylov-Schur behind an `EigenBackend` trait, Sturm count via LBLᵀ, CQC and SRSS spectrum. |
| 6 Bindings | Done | PyO3 and wasm-bindgen wrap one versioned JSON protocol. No release packaging yet. |
| 7 Building features | Done | Self-weight per load case, grounded nodal springs, rigid diaphragms. See below. |
| 8 Result store | Done | `oa-results`: SQLite consumer, rehydration, envelopes, drift, read-only SQL with row limit and an authorizer, content-hash check, run completion status (schema version 2). Timing recorded under Phase 8. |

### Phase 7: Building features (added 2026-09-14)

Three things every building model needs that the first pass lacked.

- **Self-weight.** A load case carries a per-axis multiplier, and the model
  carries gravity. Frames get a uniform global line load from density, area
  and gravity, so section-force diagrams include it. Shells get lumped nodal
  forces from the same row-sum mass used for modal analysis. Verified against
  a hand calculation, an explicit equivalent line load, and Pynite.
- **Grounded springs.** Per-node translational and rotational stiffness added
  to the diagonal. Spring reactions are reported in the reaction vector as
  minus stiffness times displacement. A spring on a restrained DOF is an
  error. Verified against a hand calculation and Pynite.
- **Rigid diaphragms.** Implemented as a master-slave transformation, not a
  penalty. The solver builds u = T q + u_prescribed where slaves of a
  diaphragm share the master's two in-plane translations and its rotation
  about the normal, offset by the lever arm n × r. Stiffness and loads are
  reduced through Tᵀ at the triplet level, so no sparse matrix product is
  needed. Slave in-plane DOFs cannot be restrained or sprung; the master must
  be an explicit node, and its unstiffened DOFs are dropped rather than
  reported unstable.

  Modal analysis with diaphragms uses the reduced mass Tᵀ M T, which is
  diagonal except for one dense block per diaphragm master coupling in-plane
  translation and rotation. That block is factored by symmetric
  eigendecomposition so the master need not sit at the centre of mass. The
  Sturm count uses the same reduced entries. Verified against the exact
  two-DOF coupled eigenproblem for offset masses, and the test confirms simple
  lumping would be wrong by more than 5 percent on that case.

Not yet done from this phase:

- Automatic master node at the centre of mass. The model layer owns that now.
- Rigid-body modes remain out of scope.

### Equilibrium check and iterative refinement (revised 2026-09-14)

The linear solve originally did one step of iterative refinement and then
rejected any solution whose residual exceeded 1e-7 relative to the load. A
27,716-node, 40-storey frame failed that check at 1e-4, and so did a small
chain of members with a 1e6 stiffness contrast. Neither was a solver
failure. On both, the products in K·u are up to 1e12 times larger than the
loads they sum to, so the residual cannot be computed below the rounding
floor no matter how many refinement steps run. The check was measuring
conditioning, not accuracy.

Two changes:

- **Backward error is the criterion.** The residual is now scaled by
  ‖K‖·‖u‖ + ‖f‖, with ‖K‖ the row-sum norm of the reduced stiffness. A
  residual at that floor means the solution is as accurate as double
  precision allows for that matrix, which is the standard definition of a
  well-solved system. The tolerance stays at 1e-7. Forward accuracy is
  still bounded by condition number times machine epsilon, and callers
  who need better must improve the model, not the solver.
- **Refinement iterates.** Up to eight steps, stopping when the residual
  is under tolerance or stops halving. This helps where factorization
  error dominates and costs nothing where it does not.

The mechanism guard is unchanged: an equilibrated inverse probe above 1e12
still rejects the system outright, which is what catches a 1e8 stiffness
contrast. The `relative_residual` reported per combination is now the
backward error. Tests: `crates/oa-core/tests/conditioning.rs`.

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
  Python parsing take a large share of API time on big models. Phase 8's
  Parquet output is that path.
- Disk-backed or envelope-only result consumers. Planned as Phase 8.
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

- Analysis emits results per combination, in requested order, through a small result-consumer trait. The default implementation collects everything in memory. The consumer runs on the calling thread while the pool solves the next combinations, so a serial writer such as SQLite overlaps with computation.
- Callers select which combinations and which output fields they want. Detailed force-diagram samples along members are generated on demand, not stored.
- Envelopes retain the governing combination for each value. This is required for design, not optional.
- Bounded concurrency: `max_in_flight` caps combinations started but not yet consumed, counting both running solves and finished results waiting for their turn. `threads` sets the worker count for the whole run, element preparation and factorization included; zero uses the caller's pool, and the most recently used pool is kept for reuse.

The memory risk is predictable. Displacements alone for a large model:

| Nodes | DOFs per node | Combinations | Bytes per value | Total |
|---------|---------------|--------------|-----------------|--------|
| 100,000 | 6             | 1,000        | 8               | 4.8 GB |

Retaining every result is therefore not part of the solver's contract. A disk-backed or envelope-only consumer can be added later without changing the analysis code.

Guardrail: in Phase 2 the consumer interface is one trait with one method, and the in-memory collector is the only implementation. Do not build the disk format or a second backend until a measured model needs it.

**Update 2026-09-14: the disk backend is now designed as Phase 8.** The
in-memory consumer stays the default and the only implementation for the
browser target.

### Result store engine

**SQLite through `rusqlite`. Decided 2026-09-14 after considering and
rejecting a columnar stack.**

Results are written once per combination in bulk and read many times as
aggregates: envelopes over all combinations, maximum drift per storey,
governing combination per DOF. A columnar engine answers those queries 10 to
100 times faster than SQLite at very large scale. That was the first
recommendation, and it was reversed for three reasons:

- **Simplicity.** `rusqlite` is one small, mature crate that builds in
  seconds. The columnar alternative is Parquet plus a query engine, and the
  only pure-Rust engine, Apache DataFusion, adds several hundred crates,
  minutes to a clean build, and monthly breaking releases.
- **Realistic scale.** The 4.8 GB figure above is a deliberate worst case. A
  large real building is around 5,000 nodes and 200 combinations, which is a
  million displacement rows and perhaps 20 million frame-force rows with
  stations. SQLite bulk-inserts over a million rows per second inside a
  transaction and answers indexed envelope queries on that volume in seconds.
- **The audience is agents, not Python.** The query surface exists so that an
  AI agent can ask open questions of the results. Agents write SQL well, and
  SQLite's SQL is sufficient. The earlier argument for Parquet, that Python
  users could read it in one line, stopped mattering once Python was confined
  to the verification harness.

Options considered:

| Option | Verdict |
|---|---|
| SQLite via `rusqlite` | Chosen. Single file, ACID, SQL for agents, tiny, also serves the model layer's command journal. |
| Parquet + DuckDB | Fastest analytics. DuckDB needs a C++ compiler to bundle and is a 50 MB binary. Rejected for cost. |
| Parquet + DataFusion | The pure-Rust columnar answer. Named as the upgrade path if a measured model outgrows SQLite. Not a dependency now. |
| Polars | Dataframe library, not a store. Rust API is secondary to Python's and churns. Rejected. |
| Key-value stores | No query capability. Rejected. |

Build note: `rusqlite`'s `bundled` feature compiles the SQLite C source and
needs a C compiler. The gcc that rustup ships for the `windows-gnu` target is
a linker only and cannot compile C; verified 2026-09-14. The older
`winsqlite3` feature that linked the SQLite shipped with Windows has been
removed from current `libsqlite3-sys`. A Windows development machine
therefore needs a real C toolchain. Resolved the same day: WinLibs MinGW-w64
(winget `BrechtSanders.WinLibs.POSIX.MSVCRT`, chosen to match the C runtime
of the `windows-gnu` target) compiles bundled SQLite 3.53.2 cleanly, and
`scripts/cargo.ps1` sets `CC` and `AR` to it when present. Release and CI
builds use `bundled` so the SQLite version is pinned.

## Phase 8: Result store

A new crate, `oa-results`, depending on `oa-core` and `rusqlite`. The solver
crate does not change.

- **SQLite consumer.** Implements `ResultConsumer`. Opens one database per
  analysis run, writes each combination inside a transaction with prepared
  statements, and commits per batch. Tables: `displacements`, `reactions`,
  `frame_end_forces`, `shell_results`, and later `frame_stations`. Every row
  carries the combination name and the entity index. A `run` table records
  the model content hash, solver version, schema version, options,
  timestamp, the combinations the run was asked for, and a completion flag
  that the last combination's transaction sets. `open` refuses a run that
  stopped early; `open_partial` exposes what finished together with the
  missing names, and the model layer refuses to attach an unfinished run.
- **Schema.** Fixed columns per table, documented in the crate, versioned
  with the same scheme as the JSON protocol. Composite index on
  `(combination, entity)` and on `(entity)` for envelopes. WAL journal mode.
- **Query layer.** Rust functions for the common questions: envelope with
  governing combination per entity, per-storey drift, and extraction by a
  list of entities, which is how groups arrive from the model layer. Each is
  a prepared SQL statement. Agents get `sql`, which prepares the statement
  under a SQLite authorizer that permits only reads and functions: `ATTACH`,
  `DETACH`, pragmas, transactions, and schema changes are refused even
  though SQLite itself classifies some of them as read-only. The raw
  connection is also exposed for trusted Rust callers.
- **Browser.** Unchanged. The in-memory consumer remains the only
  implementation for the WebAssembly target.
- **Acceptance.** Round-trip test: run the benchmark frame through the
  SQLite consumer, query the envelope through the Rust functions, and match
  it against the in-memory `Envelope`. Timing test on the 10x10x16 frame
  with 32 combinations, run in release.

  **Timing result, 2026-09-14.** The original criterion said the SQLite
  write must cost less than JSON serialization. It does not, and the
  criterion was wrong:

  | Step | Time | Size |
  |---|---|---|
  | Solve | 0.67 s | |
  | Serialize results to JSON | 0.14 s | 108 MB |
  | Write results to SQLite | 1.18 s | 53 MB |
  | Envelope query from SQLite | 0.7 ms | |

  Scaling, measured later the same day with 32 combinations except the
  last row, which has 8:

  | Nodes | DOFs | Solve | JSON | SQLite write | Envelope query |
  |---|---|---|---|---|---|
  | 6,400 | 38k | 3.97 s | 0.47 s (347 MB) | 4.17 s (172 MB) | 0.9 ms |
  | 14,553 | 87k | 11.4 s | 1.03 s (805 MB) | 9.64 s (404 MB) | 0.8 ms |
  | 27,716 | 166k | 27.2 s | 0.52 s (388 MB) | 2.88 s (198 MB) | 0.3 ms |

  Both writers are linear in row count, SQLite at a steady 230,000 rows per
  second and JSON at 780 MB per second, so the 8x ratio is constant. The
  write tracks the solve time and falls well below it as models grow,
  because the solve grows faster than linearly. Queries stay flat.

  Writing indexed rows is more work than streaming text, and the store
  costs about 1.8 solves on the smallest model above. What it buys is a sub-millisecond
  envelope query against a 53 MB file that never has to be loaded into
  memory, which is the point. The JSON path cannot answer that query
  without parsing 108 MB first. The revised criterion is that the write
  stays within 2x the solve time and envelope queries stay under 10 ms;
  both hold. Bulk-insert tuning (larger transactions, `synchronous=off`
  during the run) is available if the write ever dominates.
- **Upgrade path.** If a measured model makes envelope queries take longer
  than the analysis, add a Parquet export from the same consumer and query it
  with DataFusion. The `ResultConsumer` boundary means the analysis code
  does not change.

Not in Phase 8: modal or spectrum results, and any attempt to store the
model itself in a database.

Not in Phase 8: writing modal or spectrum results to Parquet, and any
attempt to store the model itself in a database.
