# open-analysis

An open-source structural analysis engine, written in Rust, intended as the
calculation core for an ETABS / SAP2000 style application.

The engine takes a model as flat tables of nodes, materials, sections, frames,
shells, load cases, and combinations. It runs linear static, nonlinear
(tension-only / compression-only), P-Delta, modal, and response-spectrum
analyses. All quantities are SI internally. Load combinations are solved in
parallel and streamed through a result consumer so memory use stays bounded.

The design and its rationale are in [docs/solver/PLAN.md](docs/solver/PLAN.md).

## Layout

```
crates/oa-core/     analysis engine, CLI binary `oa`, and tests
crates/oa-py/       PyO3 extension module
crates/oa-wasm/     wasm-bindgen module
python/open_analysis/   Python package wrapping the extension
scripts/            build wrapper and Pynite differential verification
examples/           sample JSON request and Python usage
docs/solver/        design plan
```

## Building

A Rust toolchain of 1.87 or later is required. If none is installed, the
wrapper script `scripts/cargo.ps1` will use a repository-local toolchain in
`.tools/` when one exists.

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
