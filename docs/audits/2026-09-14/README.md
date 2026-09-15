# Solver, model, and storage audit

Audited commit: `0856b9188665802e8da9dd268fa02d9df97133d0`.

## Summary

Eight findings: four high priority (P1), four medium priority (P2). The most
serious numerical defect returns zero support reaction for a restrained
diaphragm carrying a 1,000 N load, while reporting zero residual. Storage also
accepts an incomplete analysis as an attachable result set.

The existing native test suite passes: **51 passed, 1 ignored**. Eight additional
diagnostic probes reproduced seven of the findings below; the save interruption
risk was established by source inspection, without fault injection. Production
code was not changed.

## Findings

### 1. P1 — Transfer diaphragm forces to restrained master reactions

Location: `crates/oa-core/src/assembly.rs:480–490`, especially `ku[i] - f[i]`.

`SparseSystem::solve` calculates a restrained node's reaction from its own row
of the full stiffness matrix. Diaphragm forces are assembled at slave nodes,
so the master's row does not contain the forces transferred through the
constraint. Slave reactions are then set to zero because the slaves are
unrestrained. Those forces disappear from the output.

**Reproduction:** connect the tip of a 3 m cantilever to a fixed diaphragm
master, with normal Z, then apply 1,000 N in X to the tip. The solve succeeds,
reports zero residual, and the sum of all support reactions in X is **0 N**.
It should be **−1,000 N**. The offset master should also receive the
corresponding moment.

**Fix:** recover constraint forces and accumulate their force and moment
contributions into restrained master reactions. Check global force and moment
balance for both fixed and partially restrained masters. Apply the same
constraint-aware recovery to response-spectrum reactions, whose current
element-sum implementation has the same omission; that second path was
source-reviewed, not separately reproduced.

Probe: `restrained_diaphragm_master_loses_support_reaction`.

### 2. P1 — Include grounded springs in nonlinear equilibrium checks

Location: `crates/oa-core/src/analysis.rs:255–279`.

The solved stiffness includes `prep.springs`, but the updated internal force
used to test nonlinear/P-Delta equilibrium is assembled only from frames and
shells. The missing `k_spring * u` is treated as an imbalance, even when the
solution is in equilibrium. Increasing the iteration limit does not resolve a
constant missing force contribution.

**Reproduction:** the existing cantilever tip-spring configuration, with a
1 MN/m vertical spring and −1,000 N tip load. Linear analysis returns the
correct displacement, **−0.000529411764706 m**. Both nonlinear and P-Delta
analysis fail with a residual of **0.529411764706**. The probe uses three
iterations; this elastic configuration has no changing active set or axial
stiffness to eliminate that residual in later iterations.

**Fix:** include translational and rotational spring internal forces in the
updated equilibrium vector. Add tests combining springs with both nonlinear
methods, including diaphragm-master springs.

Probe: `nonlinear_spring_fails_despite_linear_equilibrium`.

### 3. P1 — Mark and reject incomplete persisted analysis runs

Locations: `crates/oa-results/src/lib.rs:140–173,260–262` and
`crates/oa-model/src/store.rs:58–65`.

The result store commits each combination separately. It records the model
hash and options, but no successful-completion state or enforced expected
combination set. If a later combination fails or execution stops, the earlier
combinations remain on disk. `open`, `matches`, `attach`, and envelope queries
accept that partial database without indicating incompleteness.

**Reproduction:** a cantilever with torsional end releases has two load cases:
one valid vertical load, followed by an unsupported tip torque. Analysis
returns `load on inactive node 1 DOF 3`. Reopening the database succeeds,
`matches` is true, `attach` succeeds, and the displacement envelope includes
only `good`, despite the run requesting both cases.

**Fix:** persist the expected selected combinations and a run status. Mark the
run complete only after all results commit. Refuse normal attachment/envelopes
for incomplete runs, or expose partial-result access explicitly with status.
This can retain bounded, per-combination writes.

Probe: `failed_run_reopens_and_attaches_with_partial_envelope`.

### 4. P1 — Preserve the last saved model until replacement succeeds

Locations: `crates/oa-mcp/src/lib.rs:236–242` and
`crates/oa-gui/src/document.rs:218–220`.

Both save paths call `std::fs::write` directly on the destination. Updating an
existing file truncates the last saved model before the replacement is fully
written. A disk-full error or process interruption after truncation can leave
an empty or partial JSON document and destroy the previous valid save.

**Evidence:** source inspection of both save implementations. No disk-full or
process-kill experiment was run. Neither the GUI nor MCP session currently
attaches the optional `Journal`, so those entry points have no command journal
to compensate for this failure.

**Fix:** serialize first, write and flush a temporary file in the destination
directory, then replace the destination using an appropriate atomic operation
for the platform. Preserve the prior file if any earlier step fails. Add a
failure-injection test proving the old document remains loadable.

### 5. P2 — Exclude rejected edits from the recovery journal

Location: `crates/oa-model/src/editor.rs:29–35`.

`Editor::apply` appends the command before checking whether it can apply.
Duplicate-name errors, dangling references, and failed batches therefore
leave rejected commands in the durable journal. The replay API supplies those
commands without an accepted/rejected marker. Applying the replay in order
with normal error propagation stops at the rejected edit and misses subsequent
accepted work.

**Reproduction:** accept node A; reject another node named A; accept node B.
The journal contains all three commands. Normal replay fails at the duplicate
name and recovers only A.

**Fix:** stage/validate a command before durably recording it, then publish the
accepted state, or use a transactional journal protocol that identifies
committed edits. Preserve undo/redo entries if journalling fails: those paths
currently pop their stack before the fallible append.

This finding concerns callers opting into `Editor::with_journal`; current GUI
and MCP constructors do not enable it.

Probe: `rejected_command_is_replayed_and_blocks_later_edits`.

### 6. P2 — Validate identity and allocator invariants when loading models

Locations: `crates/oa-model/src/format.rs:35` and
`crates/oa-model/src/model.rs:185–194,271–274`.

Loading only deserializes the document; compilation does not check that
`next_id` exceeds every existing ID or that IDs are unique across entity
tables. `allocate` trusts the supplied counter, and `insert` replaces any
existing entity under the allocated key.

**Reproductions:**

- A loadable, compilable model with existing node #1 and `next_id: 1` silently
  replaces node #1 on the next `insert`. Command-based callers instead receive
  already-used IDs and fail edits that should succeed.
- A node and group can share #1 in a loaded model, and compilation succeeds.
  `kind_of(1)` resolves only the node, making identity-based access ambiguous.

**Fix:** reject or normalize a stale allocator while preserving its high-water
mark; reject cross-table duplicate IDs and invalid group members on load.
Use checked arithmetic for ID exhaustion so maximum IDs cannot panic or wrap.

Probes: `loaded_allocator_can_overwrite_an_existing_node` and
`loaded_duplicate_identity_passes_compilation`.

### 7. P2 — Enforce read-only SQL beyond `Statement::readonly`

Location: `crates/oa-results/src/lib.rs:449–455`.

The SQL API treats `stmt.readonly()` as sufficient to guarantee a read-only
operation. SQLite classifies `ATTACH` as read-only for this check even though
it can create a database file and mutate connection state. This API is exposed
through MCP `query_results` as read-only SQL.

**Reproduction:** `store.sql("ATTACH DATABASE '<new path>' AS extra", 10)`
returns success and creates a new SQLite file at that path. The probe used a
fresh file under the repository's ignored `target/audit-probes` directory.
It did not modify an existing user file.

**Fix:** use a SQLite authorizer to reject attachment, detachment, unsafe
pragmas, and other side-effecting actions. Retain the statement check as an
additional guard. Add negative tests for `ATTACH` and connection-changing
statements alongside the existing INSERT/DELETE tests.

Probe: `read_only_sql_can_create_an_attached_database`.

### 8. P2 — Run element geometry validation during model compilation

Location: `crates/oa-model/src/compile.rs:308–315`.

Compilation calls the solver model's table/parameter validation, but not the
element preparation checks. Degenerate, warped, or crossed shells and invalid
local frame orientation can therefore be reported as successfully compiled.
The GUI uses compilation for its validation-problem list, so these models have
no reported geometry problem until analysis is attempted.

**Reproduction:** a shell whose four corners all reference the same node
compiles successfully. Analysis immediately rejects it with
`shell 0: degenerate corners`.

**Fix:** expose and reuse element geometry validation before compilation
returns success, preferably mapping errors back to editable entity IDs. The
existing core `validate_model_json` already invokes element preparation and
therefore has a stronger validation contract than model-layer compilation.

Probe: `compile_accepts_impossible_shell_geometry`.

## Validation and scope

Baseline command:

```powershell
.\scripts\cargo.ps1 test -p oa-core -p oa-model -p oa-results -p oa-mcp
```

Result: **51 passed; 0 failed; 1 ignored** (the ignored test is a timing
benchmark). This includes static, nonlinear, P-Delta, modal, spectrum, shell,
spring, diaphragm, pinned Pynite comparison, model command, journal, SQLite,
and MCP session tests. Missing dependencies were downloaded before execution.

The audit additionally read the core assembly/element/result-recovery code,
editable entities/commands/compiler/format/library, result store, session
integration, GUI save path, and JSON/Python/WASM wrappers. GUI execution,
Python/WASM builds, fresh external Pynite fixture generation, power-loss
durability, and large-scale resource exhaustion were not tested.

### Re-run the diagnostic probes

`probes.rs` is retained outside the ordinary test suite. It asserts the
**observed defects**, so its eight passing tests mean the reproductions
succeeded; they are not regression tests of corrected behavior.

From the repository root, copy it to a fresh temporary integration-test path:

```powershell
Copy-Item docs/audits/2026-09-14/probes.rs crates/oa-model/tests/audit_probe.rs
$env:RUST_TEST_NOCAPTURE = '1'
.\scripts\cargo.ps1 test -p oa-model --test audit_probe --offline
Remove-Item -LiteralPath crates/oa-model/tests/audit_probe.rs
```

Check that the temporary destination does not already contain your own work
before copying. The PowerShell wrapper drops bare `--`, so diagnostic output
is enabled through the environment. Probes leave their small diagnostic
databases in the ignored `target/audit-probes` directory.

Observed diagnostic result: **8 passed; 0 failed**, including nonlinear and
P-Delta residuals of `0.5294117647058827`, a diaphragm reaction sum of zero,
an attachable partial database, a stopped journal replay, allocator overwrite,
duplicate identity acceptance, SQL file creation, and accepted degenerate
shell compilation.

## Resolution (2026-09-14)

All eight findings were addressed on the same day. Re-running `probes.rs`
against the fixed tree gives **0 passed; 8 failed**, which is the expected
outcome since each probe asserts the original defect. The regular suites
gained regression tests for every finding.

| # | Change |
|---|---|
| 1 | `SparseSystem::reactions` accumulates each slave DOF's out-of-balance force into its restrained master DOFs (the restrained part of Tᵀ g); response-spectrum reactions transfer the slave element sum minus the slave's own modal inertia the same way. Tests: fixed master, partially restrained master with force and moment balance, spectrum base shear. |
| 2 | Nonlinear and P-Delta equilibrium checks add `k_spring * u` to the internal force vector. Tests: tip spring and diaphragm-master spring under both methods reproduce the linear answer. |
| 3 | Result store schema version 2 records the expected combinations and a completion flag set in the last combination's transaction. `open` refuses an unfinished run, `open_partial` exposes it with `missing_combinations`, reads on an unfinished store are refused, and `attach` refuses incomplete runs. |
| 4 | `oa_model::save_json` writes a sibling `.tmp` file, syncs it, and renames it over the destination; the MCP session and GUI use it. A failure-injection test blocks the temporary path and checks the previous document survives. |
| 5 | `Editor` applies a command before journalling it and rolls the edit back if the append fails; undo and redo keep their stack entry until journalled. Rejected commands and failed batches leave no journal entry. |
| 6 | `from_json` rejects cross-table duplicate ids, dangling group members, and an exhausted id space, and moves a stale allocator past the highest id. `compile` also reports duplicate ids. `allocate` saturates instead of wrapping. |
| 7 | `ResultStore::sql` prepares statements under a SQLite authorizer that allows only reads and functions; `ATTACH`, `DETACH`, pragmas, transactions, and schema changes fail as not authorized. Requires the `hooks` feature of `rusqlite`. |
| 8 | `compile` runs `Model::validate_frame` and `Model::validate_shell` after table validation and reports failures as entity-addressed problems. |
