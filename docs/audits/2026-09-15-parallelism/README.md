# Computation and parallelism audit

Audited commit: `3ddc21b` (clean working tree before this audit). Measurements
taken September 15, 2026 UTC / September 14 America/Chicago.

## Assessment

**Rust's concurrency is already paying off, but several substantial parts of
the analysis pipeline do not yet exploit it.** The strongest next steps are
consistent thread control, reusing element and matrix work, and a scheduler
that adapts to combination count and memory. Adding threads to every loop
would make some measured workloads slower.

The current implementation already has:

- Parallel frame and shell preparation in `Prepared::new`.
- Parallel combination solves with an immutable shared factorization for
  linear analysis, and independent systems for nonlinear/P-Delta analysis.
- Faer's threaded numeric Cholesky and solve kernels, enabled in native
  CLI, GUI, MCP, and Python builds.
- A serial, ordered result-consumer interface with a bound on combinations
  in flight. Python releases the GIL while the Rust call runs.

The solver plan's statement that assembly uses per-thread triplet buffers
is **not implemented**: `Prepared::assemble` appends to one vector serially.
Member recovery is also serial within each combination. See
[the intended design](../../solver/PLAN.md#parallelism),
[preparation and assembly](../../../crates/oa-core/src/assembly.rs), and
[combination execution](../../../crates/oa-core/src/analysis.rs).

## Measured results

Machine: AMD Ryzen 7 7800X3D, 16 logical CPUs, Windows, Rust 1.98.0,
faer 0.24.4, resolved Rayon 1.12.0. These are **new native-engine
measurements**, not a repeat of the README's Python/Pynite comparison on a
different computer. Release profile includes thin LTO and one codegen unit.

For a real thread-count comparison, the **entire analysis** runs inside a
precreated Rayon pool, with `StaticOptions.threads = 0`. Pool creation and
model construction are excluded. Static timings include the in-memory
result collection and its disposal, but exclude JSON and Python overhead.

Medians of five timed runs after warm-up (three for modal):

| Workload | 1 worker | 8 workers | 16 workers | Speedup at 8 |
|---|---:|---:|---:|---:|
| Linear 6×6×8 frame, 1 combination | 13.92 ms | 13.29 ms | 15.41 ms | 1.05× |
| Linear 6×6×8 frame, 8 combinations | 26.06 ms | 13.93 ms | 15.74 ms | 1.87× |
| Linear 6×6×8 frame, 32 combinations | 67.80 ms | 23.46 ms | 23.35 ms | 2.89× |
| Linear 10×10×16 frame, 8 combinations | 202.34 ms | 90.00 ms | 103.81 ms | 2.25× |
| P-Delta 4×4×4 frame, 8 combinations | 44.96 ms | 8.01 ms | 8.91 ms | 5.61× |
| Modal 4×4×4 frame, 12 modes | 6.28 ms | 6.34 ms | 7.03 ms | 0.99× |

Static/P-Delta rows use `max_in_flight` equal to worker count. P-Delta loads
are reduced to 1% of the benchmark loads to avoid buckling; every combination
converged in three iterations. This is not a representative survey of
nonlinear active-set behavior.

At eight workers, changing only `max_in_flight` from four to eight reduced
the 32-combination frame from **29.15 to 23.46 ms** (19.5% less time). The
large eight-combination frame went from **97.14 to 90.00 ms** (7.3% less time).

Selected isolated stage timings:

| Stage | One worker | Eight workers | Interpretation |
|---|---:|---:|---|
| Large frame: numeric factorization | 80.46 ms | 36.31 ms | Faer threading already helps: 2.22× |
| Large frame: symbolic analysis | 13.16 ms | 13.42 ms | Serial work unaffected by pool size |
| Large frame: assembly + factorization | 105.12 ms | 60.91 ms | Major shared cost before combinations |
| Large frame: state construction, existing serial loop | 6.44 ms | 6.35 ms | Repeated for each combination |
| Large frame: state construction, parallel trial | 7.80 ms | 2.82 ms | 2.25× versus the serial loop in the same 8-worker pool |
| Large frame: frame recovery, serial / parallel trial at 8 workers | — | 0.410 / 0.194 ms | Worth less than state construction |
| 1,024 DKMQ shells: assembly + factorization | 12.05 ms | 13.64 ms | More workers do not accelerate serial assembly |
| 1,024 DKMQ shells: numeric factorization | 1.64 ms | 2.19 ms | Small factor benefits from a serial policy |
| 1,024 DKMQ shells: recovery, serial / parallel trial at 8 workers | — | 0.354 / 0.119 ms | Independent loop, but modest absolute saving |

Stage samples are separate microbenchmarks with different allocation/cache
conditions. Numeric and symbolic timings are **subsets** of assembly and
factorization, not additional costs. Do not sum this table into end-to-end
timings. Parallel trials are diagnostic alternatives, not production changes.

## Ranked opportunities

### 1. Fix thread-budget scope and the benchmark's single-thread baseline

**Priority: first. Effort: small to medium. Evidence: confirmed implementation issue.**

Locations: `analysis.rs:97–108,143–172`; `assembly.rs:310,320,421`;
`scripts/benchmark_pynite.py:125–132`.

`StaticOptions.threads` creates a pool, but installs only the combination
batch into it. `Prepared::new` and the shared linear assembly/factorization
run outside that pool. Faer's high-level factor/solve functions obtain its
global parallelism setting; the default resolves the current Rayon pool's
size. Consequently `threads: 1` does not make the entire run single-threaded.

The existing Python benchmark labels this mixed execution as “OA 1 thread.”
Its “16 threads” path actually requests the current default pool, not a fixed
16-worker pool. Its timings also include Python serialization/deserialization
and a new custom pool in each nominally serial call. The measured historical
wall times remain useful, but they cannot establish whole-engine 1T scaling.

**Recommendation:** execute preparation and shared-system construction in the
selected pool too, while keeping `consumer.consume` on the original caller
thread. Reuse pools across analyses or let a reusable engine own one. Provide
the same execution settings for static, modal, and spectrum analysis. Keep
faer parallelism explicit at low-level kernels when choosing different
policies for factorization and single-column solves; do not toggle a global
setting around simultaneous requests.

For benchmarks today, install the complete in-memory analysis into a fixed
pool with `threads: 0`, as the probe does, or start separate processes with
fixed `RAYON_NUM_THREADS` values. Record JSON timings separately.

The dependency behavior was verified in the locally installed source:
`faer-0.24.4/src/lib.rs:939–947,1112–1146` and
`src/sparse/solvers.rs:95–119,223–239`. See also the primary documentation for
[Rayon pool scope](https://docs.rs/rayon/latest/rayon/struct.ThreadPool.html#method.install)
and [faer parallelism](https://docs.rs/faer/latest/faer/enum.Par.html#method.rayon).

### 2. Adapt combination scheduling to cores, work size, and memory

**Priority: high. Effort: medium. Evidence: measured scheduling opportunity.**

Locations: `analysis.rs:64–65,159–178`; `results.rs:46–57`.

The default `max_in_flight = 4` allows at most four combination tasks,
regardless of CPU count. Each batch waits for its slowest combination, then
all results are consumed serially before any work in the next batch starts.
Different nonlinear convergence times can leave workers idle behind a slow
combination. The bound is valuable; simply making it unlimited is unsuitable.

**Recommendation:** support an automatic memory-aware limit, retaining an
explicit override. Use a rolling bounded window of tasks and an ordered
completion buffer. Admit replacement work as ordered consumption releases
memory credits, with any extra reorder capacity explicitly budgeted.
Bound both active tasks and completed results waiting for their turn. Stop
admitting work on error/cancellation; preserve the current ordering and
consumer-failure behavior.

Use outer combination parallelism for many small independent systems;
allocate more work inside one system for few large combinations. Same-pool
nested Rayon work shares its worker set; it does not inherently create
16×16 OS threads. Separate per-request pools can oversubscribe the machine.

The measurements justify tuning, not setting every request to 16 workers.
Eight workers often beat 16 on this machine. The in-memory consumer still
retains all completed results; `max_in_flight` only bounds transient work.

### 3. Cache invariant element state, then parallelize changing states

**Priority: high. Effort: medium. Evidence: measured serial hotspot.**

Locations: `assembly.rs:91–107`; `element/frame.rs:173–233`;
`analysis.rs:150–155,198,247`; `element/shell.rs:342–350`.

Every `Prepared::states` call serially reconstructs each frame's stiffness,
release condensation, and transformed stiffness. Linear analysis does this
once for the shared matrix and again for every combination, even though
axial force is zero and only loading changes. P-Delta does it for both the
current and updated states each iteration. `ShellElement::global_k` also
repeats a constant matrix transformation on every call.

**Recommendation:** split load-independent stiffness/release data from
load-dependent vectors. Cache elastic condensed stiffness, release recovery
operators, shell global stiffness and center recovery operators. Compute
changing frame states with indexed parallel iteration above a size threshold.
Keep matrix calculations inside an individual 12×12 or 24×24 element local;
parallelize over elements rather than within each small matrix.

For 5,456 frames, a direct parallel-state trial reduced 6.35 to 2.82 ms at
eight workers. This is an isolated kernel result: nested use inside already
parallel combinations needs an end-to-end benchmark. Caching could avoid
more work than threading alone. Preserve the load-dependent check for a
load acting on an unsupported released mode. Geometric stiffness and release
condensation must be refreshed when axial forces change in P-Delta.

### 4. Reuse matrix structure and parallelize assembly without shared writes

**Priority: high, especially for shells and nonlinear runs. Effort: medium to large.**

Locations: `assembly.rs:108–120,170–181,242–310`;
`analysis.rs:200–203`; `modal.rs:585–595`.

Assembly serially emits element triplets, constructs the full compressed
matrix, transforms entries for constraints, and constructs the reduced
compressed matrix. `sp_cholesky` repeats symbolic ordering/analysis whenever
a new `SparseSystem` is created. Symbolic analysis determines the storage
and dependencies of the factor; numerical factorization computes its values.

**Recommendation:** generate element triplets into chunk-local buffers and
merge in fixed element order. Reuse a compiled mapping from element entries
to compressed-matrix positions, plus constraint projections and symbolic
factorization where the pattern is unchanged. Faer's `SymbolicLlt` and
`Llt::try_new_with_symbolic` support this split. Preserve numeric reassembly,
factorization, stability detection, and residual validation.

For nonlinear active-set changes, use a retained union pattern only if the
reduced DOF mapping remains valid, or cache by active topology. The current
assembler drops zero entries and unstiffened DOFs, so pattern reuse cannot
be unconditional. P-Delta numeric changes still require new numeric factors.

At eight workers, symbolic analysis alone cost 13.42 ms per large-frame
factorization. Shell assembly/factorization cost 13.64 ms while its numeric
factor cost only 2.19 ms. These identify substantial work outside the threaded
numeric factorization. Keep stable accumulation order or tolerance-based
validation; atomics/locks on each matrix entry would introduce contention.

### 5. Make modal execution and block solves parallel-aware

**Priority: high for dynamics workloads. Effort: medium to large.**

Locations: `modal.rs:19–29,98–109,128–139,314–328,388–471,629–642`;
`assembly.rs:416–424`.

The Krylov-Schur driver and Sturm numeric factorization explicitly receive
`Par::Seq`. `MassOperator::apply` ignores its `Par` argument, processes input
columns serially, and allocates temporary vectors for every column. Dense
fallback constructs its operator one column at a time. Each recovered mode
also performs a separate one-column solve.

**Recommendation:** pass a shared execution policy through scratch sizing and
computation. Add a multiple-right-hand-side solve to `SparseSystem`; use it
for operator blocks and bounded mode-recovery batches. Reuse scratch buffers.
Parallelize independent mode recovery and pairwise orthogonality checks,
then aggregate diagnostics in mode order. Retain serial thresholds for small
problems and benchmark on substantially larger modal models.

Krylov iterations have dependencies; they cannot be parallelized by treating
successive iterations or requested modes as independent solves. Faer's
high-level stiffness solves may already use threads even when the driver
receives `Par::Seq`, so “modal is completely single-threaded” would be wrong.
The measured small 12-mode case showed no improvement with more workers;
this audit does not establish a speedup for a revised eigensolver.

### 6. Parallelize spectrum responses and simplify SRSS

**Priority: high for many modes/large output sets. Effort: small to medium.**

Locations: `spectrum.rs:98,125–153,140–229,230–251,338–357`.

After modal analysis, each mode's amplitude, member response and support
reaction can be recovered independently, but the loop is serial. Combination
of responses is also serial over all output components. The generic quadratic
combiner loops over every pair of modes even for SRSS, whose off-diagonal
correlations are all zero.

**Recommendation:** parallelize mode recovery with bounded concurrency, and
parallelize CQC over output-component tiles, keeping a fixed summation order
inside each component. Specialize SRSS to `sqrt(sum(response_i²))`: work drops
from O(outputs × modes²) to O(outputs × modes). SRSS can accumulate incrementally;
CQC needs access to cross-mode terms, so tile storage and work accordingly.

Share preparation with modal analysis instead of rebuilding `Prepared`.
Expose reuse of a modal basis across spectrum directions/curves. Avoid full
shell stress recovery when only reaction assembly needs element end forces.
Preserve signed CQC cross terms and sum base reactions per mode before
combining; summing already-combined component peaks is incorrect.

This opportunity is established by code inspection and operation count;
no revised CQC/SRSS implementation was benchmarked.

### 7. Pipeline results and reduce repeated work at API boundaries

**Priority: medium; higher for large exports. Effort: medium.**

Locations: `analysis.rs:175–177,206,381`;
`oa-results/src/lib.rs:279–365`; `io.rs:55–56`;
`oa-py/src/lib.rs:4–6`; `oa-model/src/api.rs:51–57`;
`oa-model/src/compile.rs:323–336`; `model.rs:665–671`.

Consumers run only after a whole batch completes. SQLite correctly uses one
writer and transactions, but writing a batch currently prevents calculating
the next one. JSON serializes the entire result after calculation. Measured
Rust JSON formatting alone took **28.40 ms for 27.09 MB** of large-frame
results, before Python decoding or transport costs.

**Recommendation:** let a bounded compute producer overlap with the calling
thread's ordered consumer. Keep the SQLite connection on its owning thread;
multiple competing database writers are unlikely to help. Add typed/binary
array outputs for Python and streaming/chunked serialization for large exports.
Continue to support the current JSON interface.

Other avoidable costs to address alongside threading:

- Static linear analysis always recovers frame results even when frame
  output is off. Skip it when unused; nonlinear activation still needs forces.
- Cache equivalent loads per load case before combining them. For linear
  analysis with many combinations but few independent cases, evaluate solving
  a load-case basis and superposing responses. Include prescribed-displacement
  effects once as an affine baseline; this does not apply to nonlinear/P-Delta.
- The model JSON API serializes an already-built request and immediately
  parses it in `oa_core::solve_json`; call the typed API instead.
- Compilation validates geometry by constructing and discarding every element,
  serially; analysis constructs the elements again. Separate geometry checks
  from expensive matrix preparation or safely reuse prepared data. Keep
  validation and entity-addressed errors intact.

Serial sparse matrix/vector multiplication (`assembly.rs:524–531`) is a later
target if larger models make it material. Current column-wise accumulation
writes shared output rows; use row-owned CSR/gather work or bounded local
reductions, not a naive parallel loop over columns. It cost only 0.070 ms on
the measured large frame, so prioritize the measured larger costs first.

### 8. Run desktop and MCP analysis as background jobs

**Priority: high for responsiveness, separate from solver throughput. Effort: medium.**

Locations: `oa-gui/src/document.rs:171–187`;
`oa-gui/src/workspace.rs:635–638`;
`oa-mcp/src/main.rs:25–31,268–283`.

The GUI analyzes synchronously inside a document update. MCP invokes analysis
inside an async handler while holding the session mutex. Internal Rayon work
does not make either calling operation nonblocking.

**Recommendation:** snapshot the compiled model and revision, dispatch CPU
work to the engine/background executor, and publish results back only when
the revision still matches. Keep session locking short; expose progress and
cooperative cancellation at combination/iteration boundaries. A background
job should preserve errors and incomplete-result handling. This allows UI
interaction and MCP status operations during analysis, without promising a
faster individual factorization.

## Suggested implementation sequence

1. Repair thread-budget scope and benchmark labels; add per-stage timing,
   pool size and in-flight reporting. Establish true 1/4/8/16-worker baselines.
2. Cache invariant frame/shell data and parallelize substantial element loops.
   Add a memory-aware combination limit with an explicit user override.
3. Cache symbolic structure; implement deterministic chunked assembly. Measure
   both few-large-system and many-small-system scheduling policies.
4. Add spectrum mode/component parallelism and the SRSS specialization; then
   block modal solves and threaded Sturm kernels with size thresholds.
5. Pipeline consumers, improve Python transfer, and move application analysis
   into revision-checked background jobs.

Preserve equilibrium residuals, mechanisms/buckling rejection, release-load
checks, spring/diaphragm reactions, output ordering, consumer failure behavior,
and modal orthogonality/Sturm validation. Parallel reductions can change
floating-point rounding; check physical invariants and numerical tolerances.

Memory accounting should include shared matrices, per-combination states,
numeric factors and scratch, pending results, and results retained by the
consumer. Increasing workers is useful only when sufficient independent work
and memory are available. Browser/WASM remains intentionally serial; browser
workers and shared-memory deployment would be a separate feature project.

## Reproduction and scope

- Baseline: `cargo test -p oa-core --offline` — **38 passed**.
- Release diagnostic: **1 passed**. It imports the unchanged engine source
  into an integration harness to access private stage functions. The static
  matrix compared all node displacements against a whole-run one-worker
  baseline; maximum relative discrepancy was **1.27e-15**. Parallel state
  trials also checked stiffness matrices exactly.
- See [raw measurements](results.json), [probe source](probe.rs), and
  [the runner](run.ps1). From the repository root:

  ```powershell
  & ./docs/audits/2026-09-15-parallelism/run.ps1
  ```

The runner installs a temporary integration harness, removes that exact file
afterward, and leaves new measurements in `target/parallelism-audit.json`.
The checked-in measurements are preserved. Probe stages include allocation
and disposal; extremely short stages are timer/cache sensitive. Tests were
run sequentially with no intentional competing benchmark. CPU affinity and
power state were not controlled; samples are retained to show variability.
Configurations ran in fixed order, so small timing differences should not
be treated as statistically established improvements.

Reviewed computation and integration paths across all seven crates. Runtime
measurement covered regular frame static/P-Delta cases, one shell mesh,
one small modal model, and Rust result JSON formatting. This was not a
large-scale memory/NUMA study, a GUI responsiveness test, a Python comparison,
or a benchmark of revised assembly, modal, spectrum, or SQLite implementations.
No production source files were changed.

## Resolution (2026-09-15)

Addressed in the `perf/audit-2026-09-15-parallelism` branch. Every code
claim in the audit was confirmed by reading the cited lines before changing
anything. The probe is a benchmark, not a defect assertion, so the check
after the changes is a rerun rather than an expected failure: `probe.rs` was
adapted to the changed internal API (cached loads and stiffness, a P-Delta
progress line, and named pool threads) and rerun on the same machine with
default thread stacks. New medians are in [results-after.json](results-after.json);
the originals in [results.json](results.json) are untouched.

| # | Outcome |
|---|---|
| 1 | Done. `StaticOptions.threads` now scopes the whole run: element preparation and the shared assembly and factorization run inside the selected pool, so faer's kernels honour the budget. Pools are cached by size and reused across calls. `ModalOptions.threads` gives modal and spectrum analysis the same control. `scripts/benchmark_pynite.py` labels its columns by the real worker count, and the README notes that the historical "1 thread" column was mixed execution. |
| 2 | Done for the scheduling half. Combinations run through a rolling window: `max_in_flight` bounds running solves plus finished results waiting for their turn, replacements are admitted as ordered consumption frees slots, and the first error in order stops admission. The consumer runs on the calling thread while the pool keeps solving. A caller that is already a pool worker falls back to ordered batches it helps compute, because a worker blocked on a channel starves Rayon's wake-up logic. Not done: a memory-aware automatic limit; the default stays at four with an explicit override. |
| 3 | Done. Frame stiffness, release condensation and the release recovery operator are split from the load-dependent state and computed once per member at zero axial force in `Prepared::new`; a per-combination state borrows it through a pointer-sized handle and only a nonzero axial force condenses afresh. Shell global stiffness and centre recovery operators are cached once. Equivalent loads are cached per load case and combined by factor. Frame recovery is skipped for linear runs with frame output off. |
| 4 | Done for assembly: element triplets are generated in chunk-local buffers in parallel and concatenated in element order, so the summation order is scheduling-independent. Not done: reuse of the symbolic factorization across nonlinear iterations, since the reduced pattern changes with the active set. |
| 5 | Not done. Modal kernels still receive `Par::Seq` and the measured 12-mode case showed no gain from more workers; this needs a larger modal benchmark first. Modal analysis does gain the thread budget and the shared preparation. |
| 6 | Done. Spectrum analysis shares one preparation with the modal solve, recovers mode responses in parallel in mode order, tiles the quadratic combination across output components, and specialises SRSS to the diagonal sum. |
| 7 | Partly done: consumer overlap (item 2), skipped frame recovery and cached case loads (item 3), and the model API calls the typed solver instead of serialising a request it immediately parses. Not done: typed or streaming result transfer for Python and large exports; geometry validation in `compile` still constructs and discards elements. |
| 8 | Not done. Moving GUI and MCP analysis to revision-checked background jobs is application work outside the solver and is left for a separate change. |

Measured on the audit machine, medians of five, whole analysis inside a
precreated pool as in the original table:

| Workload | Workers, in flight | Before | After |
|---|---|---:|---:|
| Linear 6×6×8 frame, 8 combinations | 1, 1 | 26.06 ms | 14.74 ms |
| Linear 6×6×8 frame, 8 combinations | 8, 8 | 13.93 ms | 10.99 ms |
| Linear 6×6×8 frame, 32 combinations | 1, 1 | 67.80 ms | 25.93 ms |
| Linear 6×6×8 frame, 32 combinations | 8, 8 | 23.46 ms | 12.74 ms |
| Linear 10×10×16 frame, 8 combinations | 1, 1 | 202.34 ms | 155.40 ms |
| Linear 10×10×16 frame, 8 combinations | 8, 8 | 90.00 ms | 75.21 ms |
| P-Delta 4×4×4 frame, 8 combinations | 1, 1 | 44.96 ms | 35.39 ms |
| P-Delta 4×4×4 frame, 8 combinations | 8, 8 | 8.01 ms | 5.18 ms |
| `threads: 1` option, 6×6×8, 8 combinations | — | 28.28 ms | 14.09 ms |
| `threads: 1` option, 10×10×16, 8 combinations | — | 172.01 ms | 150.35 ms |
| Modal 4×4×4 frame, 12 modes | 1 | 6.28 ms | 6.11 ms |

Stage timings that changed: large-frame state construction fell from
6.44 ms to 0.54 ms per call at one worker, shell recovery on 1,024 DKMQ
shells from 0.354 ms to 0.19 ms, and shell assembly plus factorization from
12.05 ms to 10.56 ms. Preparation of the large frame rose from 7.98 ms to
13.03 ms because it now also builds the elastic stiffness, shell stiffness
and per-case loads that used to be repeated per combination. The
`threads: 1` rows are now genuinely single-threaded, factorization
included, so their improvement is despite doing less in parallel. The
16-worker rows remain noisy on this 8-core, 16-thread CPU; one of them
(6×6×8, 8 combinations, 4 in flight) reads 34 ms inside the full probe
sequence but 14 ms when that configuration is run on its own.

Two problems found while measuring, both fixed before merge: a first
version of the window admitted work before draining ordered results and
could block on an empty channel, and caching the 24×24 shell stiffness
inside the element struct made parallel element construction overflow a
default worker stack on the plate at 16 workers, so the cache lives on the
preparation instead. Regression tests cover ordered consumption under the
window, consumer and combination errors stopping after an ordered prefix,
frame output off under nonlinear activation, the thread budget for modal and
spectrum analysis, and SRSS against the generic combiner.

