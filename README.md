# open-analysis

An open-source structural analysis engine, written in Rust, intended as the
calculation core for an ETABS / SAP2000 style application.

The engine takes a model as flat tables of nodes, materials, sections, frames,
shells, diaphragms, load cases, and combinations. Nodes can carry restraints,
prescribed displacements, lumped mass, and grounded springs. Load cases can
include self-weight. It runs linear static, nonlinear (tension-only /
compression-only), P-Delta, modal, and response-spectrum analyses. All
quantities are SI internally; the desktop GUI, the MCP server, and the
bundled library present US customary units (kip, ft, in) and convert at
that boundary, with SI display planned as a second unit table. Load
combinations are solved in parallel and streamed through a result consumer
so memory use stays bounded.

The solver design and its rationale are in
[docs/solver/PLAN.md](docs/solver/PLAN.md). The editable model layer that will
sit above it is planned in [docs/model/PLAN.md](docs/model/PLAN.md).

## Layout

```
crates/oa-core/     analysis engine, CLI binary `oa`, and tests
crates/oa-results/  SQLite result store: consumer, envelopes, read-only SQL
crates/oa-model/    editable model: stable ids, names, groups, commands with
                    undo, versioned file format, libraries, command journal
crates/oa-mcp/      MCP server exposing the model layer and results to AI agents
crates/oa-gui/      desktop viewer and editor on GPUI Kit: 3D view, model tree,
                    property editing with undo, static analysis, deformed shape
crates/oa-py/       PyO3 extension module (verification harness)
crates/oa-wasm/     wasm-bindgen module: solver plus model-layer entry points
python/open_analysis/   Python package wrapping the extension
scripts/            build wrapper, Pynite verification, benchmark
examples/           sample JSON request and Python usage
docs/solver/        solver design plan and benchmarks
docs/model/         model layer design plan
docs/gui/           desktop GUI design plan and status
```

The model layer is the intended surface for both a GUI and AI agents: every
edit is a JSON command that returns its inverse, validation problems name
entities rather than indices, and results are keyed by a content hash of the
compiled model so stale results are refused.

## Building

A Rust toolchain of 1.87 or later is required. If none is installed, the
wrapper script `scripts/cargo.ps1` will use a repository-local toolchain in
`.tools/` when one exists.

The result store crate bundles SQLite, which needs a C compiler. The gcc that
rustup ships for the `windows-gnu` target is a linker only. On Windows,
install MinGW-w64 and the wrapper script will find it:

```powershell
winget install --id BrechtSanders.WinLibs.POSIX.MSVCRT -e
```

Pick the MSVCRT variant, since that is the C runtime the `windows-gnu` Rust
target links against. The wrapper also links with that toolchain rather than
rustup's bundled one, because the bundled `dlltool` has no assembler and
cannot build the import libraries that `tokio` and friends need.

```bash
cargo build -p oa-core
cargo test -p oa-core
```

To lint with warnings as errors on PowerShell, pass the flag through the
environment because the wrapper drops a bare `--`:

```powershell
$env:RUSTFLAGS='-D warnings'; .\scripts\cargo.ps1 clippy -p oa-core --all-targets
```

### CLI

The `oa` binary reads a JSON analysis request from a file or stdin and writes
JSON results to stdout:

```bash
cargo run -p oa-core --bin oa -- examples/cantilever.json
```

### Desktop GUI

`oa-gui` is an ETABS-style editor built on [GPUI Kit](https://gpui-kit.com).
It opens with an example frame; use File to start empty or open a saved
`.oa.json` model. The window has a menu bar, a toolbar, the model tree on the
left, the 3D view in the middle, and the property panel on the right.

```bash
cargo run -p oa-gui
```

In the 3D view, right-drag orbits, shift+right-drag or middle-drag pans, the
wheel zooms about the cursor, click selects, and shift+click extends the
selection. The property panel edits the selected entity through model
commands, so every change can be undone. Analyze > Run static analysis solves
every combination and draws the deformed shape. The design and the list of
what is and is not implemented are in [docs/gui/PLAN.md](docs/gui/PLAN.md).

### MCP server for agents

`oa-mcp` speaks the Model Context Protocol over stdio. Build it and point
an MCP client at the binary:

```bash
cargo build --release -p oa-mcp
```

```json
{ "mcpServers": { "open-analysis": { "command": "target/release/oa-mcp" } } }
```

An agent then calls `describe_model`, reads `command_reference`, builds a
model with `apply_commands`, runs `compile` and `analyze`, and asks for
`envelope`, `group_envelope`, `drift`, or `query_results`. Every edit
returns its inverse, so `undo` and `redo` work, and any edit discards
results so stale numbers can never be read back.

### Python

```bash
pip install maturin
maturin develop --manifest-path crates/oa-py/Cargo.toml
python examples/analyze.py
```

### WebAssembly

```bash
rustup target add wasm32-unknown-unknown
cargo build -p oa-wasm --target wasm32-unknown-unknown --release
```

The WebAssembly build is serial. The `parallel` feature of `oa-core` is off for
that target.

## Verification

Three layers of tests run under `cargo test -p oa-core`:

- Closed-form benchmarks for frames, plates, modal analysis, and response
  spectrum.
- Differential acceptance against pinned reference values generated from
  Pynite. These run without Python.
- Protocol and result-consumer tests.

To regenerate the Pynite fixtures, build the Python extension and run:

```bash
python scripts/verify_pynite.py --write-fixtures
```

This requires Pynite 3.2.0, NumPy, and SciPy on the Python path.

## Performance against Pynite

`scripts/benchmark_pynite.py` builds the same regular 3D moment frame in
Pynite and in open-analysis, runs linear static analysis over the same load
combinations, checks that every displacement agrees, and times both. The
frames have 6 m bays, 4 m storeys, fixed bases, a distributed gravity case on
every beam, a lateral nodal case on every floor node, and combinations that
mix the two with varying factors.

Measured 2026-09-14 on an Intel Core Ultra 9 386H (16 cores, 32 GB), Windows
11, Python 3.12, NumPy 2.5, SciPy sparse solver, Pynite 3.2.0, open-analysis
0.1.0 release build. Raw numbers are in
[docs/solver/benchmarks/pynite-2026-09-14.json](docs/solver/benchmarks/pynite-2026-09-14.json).

**Model size sweep, 8 load combinations**

| Frame | Nodes | DOFs | Pynite | OA 1 thread | OA 16 threads | Speedup 1T | Speedup 16T | Max rel diff |
|---|---|---|---|---|---|---|---|---|
| 2x2x2 | 27 | 162 | 0.24 s | 0.005 s | 0.005 s | 51x | 48x | 3.1e-13 |
| 4x4x4 | 125 | 750 | 0.28 s | 0.031 s | 0.032 s | 9x | 9x | 4.0e-13 |
| 6x6x8 | 441 | 2,646 | 1.47 s | 0.177 s | 0.134 s | 8x | 11x | 5.2e-13 |
| 8x8x12 | 1,053 | 6,318 | 9.39 s | 0.548 s | 0.475 s | 17x | 20x | 1.6e-12 |
| 10x10x16 | 2,057 | 12,342 | 22.95 s | 1.182 s | 1.003 s | 19x | 23x | 3.9e-12 |

**Combination count sweep, 6x6x8 frame (2,646 DOFs)**

| Combos | Pynite | OA 1 thread | OA 16 threads | Speedup 1T | Speedup 16T |
|---|---|---|---|---|---|
| 1 | 0.33 s | 0.063 s | 0.065 s | 5x | 5x |
| 8 | 1.60 s | 0.137 s | 0.130 s | 12x | 12x |
| 32 | 5.27 s | 0.410 s | 0.311 s | 13x | 17x |

**How to read these numbers**

- **Pynite time** is `analyze_linear` only. Model construction is excluded,
  though Pynite defers most element preparation into analyze.
- **open-analysis time** is the full Python API call: serialize the model to
  JSON, parse it in Rust, prepare elements, factor, solve every combination,
  serialize results, and parse them back in Python. On the 10x10x16 frame
  about 0.3 s of the 1.0 s is Python's `json.loads` of a 27 MB result string,
  and the Rust side spends a further fraction formatting it. The engine itself
  is faster than the table shows. A binary result path would close that gap.
- **The "1 thread" column predates the thread-budget fix of 2026-09-15.**
  At the time, `threads: 1` confined only the combination solves to one
  worker; element preparation and the shared factorization still ran on the
  default pool. Treat that column as "one-thread combinations" rather than a
  whole-engine serial baseline. The script now runs the whole engine on the
  requested worker count and labels the parallel column by CPU count.
- **Parallelism helps less than you might expect for linear static.** The
  stiffness matrix is factored once and shared. Each combination is only a
  back-substitution and force recovery, so 8 combinations do not give 16 cores
  much to do. The gain grows with combination count, as the 32-combination row
  shows, and would be larger still for P-Delta and tension-only analyses where
  every combination factors its own matrix.
- **Max rel diff** is the largest displacement difference between the two
  tools across all nodes and combinations, relative to the largest
  displacement. Both tools solve the same equations to machine precision.
- Pynite's time grows faster with model size because its assembly and
  fixed-end-force loops run in Python. open-analysis time grows roughly
  linearly with DOFs at these sizes.

To reproduce, build the release extension and run the script with a Pynite
checkout on the path:

```bash
cargo build --release -p oa-py
python scripts/benchmark_pynite.py --reference path/to/Pynite --json results.json
```

## Attribution

See [THIRD_PARTY.md](THIRD_PARTY.md).
