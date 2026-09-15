# Review of parallelism PR #3

Reviewed on 2026-09-15. **Recommendation: request changes for three regressions.**

- PR: [Address the 2026-09-15 parallelism audit](https://github.com/andrewcoul/open-analysis/pull/3).
- Head: `0a7a5f635bdcc3ad8300efffefaacd19910a3afc`.
- Base: `3ddc21ba9419d140ac659423d42ba1d763afbdcd`.
- Reviewed the complete source diff and surrounding static, modal, spectrum,
  assembly, element, protocol, and model API paths. Also inspected the new
  tests and the benchmark methodology.

## 1. P1: A worker panic deadlocks the result receiver

Location: [analysis.rs, lines 188–200](https://github.com/andrewcoul/open-analysis/blob/0a7a5f635bdcc3ad8300efffefaacd19910a3afc/crates/oa-core/src/analysis.rs#L188-L200).

If `work(i)` unwinds, the worker never sends its completion. Rayon catches the
panic at the scope boundary, but the scope cannot finish while its calling
closure is blocked in `rx.recv()`. The original sender at line 166 remains
alive, so disconnect detection cannot release that receiver either. An
internal numerical-library panic therefore becomes an indefinite hang instead
of reaching the caller. This affects the new rolling-window path entered from
outside a Rayon pool.

**Reproduction:** two valid cantilever combinations, `threads: 1`, and
`max_in_flight: 1`. The first consumer callback calls faer's public
`disable_global_parallelism()` solely to inject a deterministic panic into the
next backsolve. No production source is modified. The base propagates the
panic into `catch_unwind` immediately; the PR prints the worker panic and stays
blocked until the parent test terminates the child after five seconds.

This is a fault-injection test of panic handling, not a claim that ordinary
cantilever inputs cause a numerical panic. Returned solver and consumer
`Error` values are covered by the passing existing tests.

**Fix:** guarantee a completion or failure notification even during worker
unwinding, then propagate the panic or return an explicit failure while allowing
the scope to finish. Merely adding a disconnect error to `recv()` is insufficient
while the original sender remains alive.

## 2. P2: Eager dense load caches defeat bounded-memory analysis

Location: [assembly.rs, lines 74–82](https://github.com/andrewcoul/open-analysis/blob/0a7a5f635bdcc3ad8300efffefaacd19910a3afc/crates/oa-core/src/assembly.rs#L74-L82).

Preparation now expands every load case into full nodal, member, and shell
arrays before selecting combinations. Each case retains approximately
`8 × (6 × nodes + 12 × frames + shells)` bytes, even when it contains only one
nodal load or is unused by the requested analysis. This storage is independent
of `max_in_flight`; modal analysis and `validate_model_json` also construct it.

**Reproduction:** 5,000 fixed nodes, one frame, 500 cases containing one nodal
load each, and a single combination using only case zero. Run with one worker,
one combination in flight, and all result fields disabled. The deliberately
simple structure isolates allocation from factorization and result storage.
An instrumented allocator measures additional live heap after the model is
built and the worker pool is warmed:

| Revision | Additional peak heap |
|---|---:|
| Base | 5,062,233 bytes |
| PR | 125,168,297 bytes |

The roughly 120 MB increase is explained by 500 dense nodal arrays. A connected
frame model has the same nodal cost plus the member arrays. At larger case and
element counts this can exhaust memory despite selecting a single combination.

**Fix:** preserve sparse per-case storage or use a bounded cache; calculate only
needed cases, and omit static-load caches from modal and validation preparation.
Selecting only needed cases helps the subset case but does not by itself bound
memory when all cases are requested.

## 3. P2: The pool cache retains every historical thread budget

Location: [exec.rs, lines 36–51](https://github.com/andrewcoul/open-analysis/blob/0a7a5f635bdcc3ad8300efffefaacd19910a3afc/crates/oa-core/src/exec.rs#L36-L51).

The process-global map stores a strong `Arc` for every distinct nonzero
`threads` value, with no capacity or eviction. Completing an analysis therefore
never releases any of those pools. In a persistent Python, GUI, or MCP process,
trying different budgets accumulates idle OS workers and their stacks. The
requested budget bounds the current pool but not the retained worker population.

**Reproduction:** after warming the global pool, run a tiny analysis sequentially
with budgets 1 through 8. These budgets retain 36 workers in total. In the
combined probe run, earlier control tests had already cached budgets 1 and 2;
the resource probe consequently measured another 33 live threads, from 24 to
57. All analysis calls had completed. The base stayed at 21 threads before and
after the same requests, releasing its temporary pools. The probe allows enough
retained workers for the largest requested pool,
so it does not require removing caching altogether.

**Fix:** bound retained pools or total cached workers and evict unused pools.
Keep an active run's `Arc` alive while allowing inactive historical budgets to
be dropped.

## Verification

Existing PR tests:

- `oa-core`, default parallel feature: **44 passed**.
- `oa-core`, `--no-default-features`: **44 passed**.
- `oa-model`, `oa-results`, `oa-mcp`: **24 passed**, one timing test ignored.
- This includes the pinned Pynite acceptance fixtures, frame/release/P-Delta
  checks, shell benchmarks, spring/diaphragm equilibrium, and spectrum checks.
- Additional nested-pool control: one- and two-worker outer pools, requested
  budgets zero/one/two, and an `Rc`-holding consumer all complete; callbacks
  stay on their invoking thread.

The review probes are in [probe.rs](probe.rs). They use public APIs and run
unchanged at both revisions. Three regression assertions fail at the PR head
and pass at the base. The child-process helper is not a separate correctness
claim. Worker-panic testing kills only its own child process after a bounded
timeout. OS thread-count measurement is Windows-only; the memory and panic
probes are portable native Rust tests.

The broader crate tests required the installed MinGW toolchain; the first
sandboxed attempt could not use the full compiler, and the retry with access to
it passed. No production source was edited for this review.

### Reproduce

Use separate worktrees at the two commits above. From the repository containing
this audit:

```powershell
./docs/audits/2026-09-15-parallelism-pr-review/run.ps1 -Checkout .tools/pr3-audit
./docs/audits/2026-09-15-parallelism-pr-review/run.ps1 -Checkout .tools/pr3-base
```

The runner uses the repository's Cargo wrapper, runs probes sequentially, and
gives each checkout its own build directory. It temporarily installs the probe
as `crates/oa-core/tests/pr3_review.rs` and removes only an unchanged file that
it created. A nonzero exit at the reviewed PR head is expected.

## Limits

No normal-input numerical regression was found in the exercised cases. The
audit does not establish numerical correctness for every structure. The serial
feature was tested natively; no browser/WASM runtime test was performed.

The PR's reported speedups were not independently rebenchmarked. Its main
before/after table wraps analysis in precreated Rayon pools, which exercises the
ordered-batch fallback rather than the new external-caller rolling window.
Those numbers do not measure the window's throughput or consumer overlap.
The existing modal/thread-budget test compares answers; it does not directly
count workers. Source inspection supports the chosen-pool routing under the
default faer configuration.
