"""Wall-clock comparison of Pynite and open-analysis on regular moment frames.

Builds identical 3D moment frames (bays_x x bays_y x stories) in both tools,
runs linear static analysis over the same load combinations, checks that the
displacements agree, and prints a Markdown table plus JSON.

Run after `cargo build --release -p oa-py`:
    python scripts/benchmark_pynite.py --reference .tools/Pynite
Optional: --extension PATH, --sizes 2x2x2,4x4x4, --combos 8, --json OUT.json,
--pynite-limit SECONDS (skip Pynite at larger sizes once it exceeds this).
"""
import argparse
import importlib.machinery
import importlib.util
import json
import os
import platform
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--reference", type=Path)
parser.add_argument("--extension", type=Path, default=ROOT / "target/release/_oa.dll")
parser.add_argument("--sizes", default="2x2x2,4x4x4,6x6x8,8x8x12,10x10x16")
parser.add_argument("--combos", type=int, default=8)
parser.add_argument("--combo-sweep", default="6x6x8:1,8,32")
parser.add_argument("--pynite-limit", type=float, default=900.0)
parser.add_argument("--json", type=Path)
args = parser.parse_args()
if (ROOT / ".tools/python-deps").exists():
    sys.path.insert(0, str(ROOT / ".tools/python-deps"))
if args.reference:
    sys.path.insert(0, str(args.reference.resolve()))
import numpy as np
from Pynite import FEModel3D

extension = str(args.extension.resolve())
loader = importlib.machinery.ExtensionFileLoader("_oa", extension)
spec = importlib.util.spec_from_file_location("_oa", extension, loader=loader)
oa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(oa)

E, NU, RHO = 200e9, 0.3, 7850.0
SECTION = dict(area=0.01, iy=2e-5, iz=4e-5, torsion=1e-5)
BAY, STORY = 6.0, 4.0
W_DEAD = -10_000.0  # N/m on every beam, global Y (down)
P_WIND = 5_000.0  # N per floor node, global X


def frame_model(bx, by, stories, combos):
    """Regular moment frame: Y up, columns and beams in both plan directions."""
    nx, ny, nz = bx + 1, by + 1, stories + 1
    nodes, index = [], {}
    for k in range(nz):
        for j in range(ny):
            for i in range(nx):
                index[(i, j, k)] = len(nodes)
                nodes.append({"position": [i * BAY, k * STORY, j * BAY], "restrained": [k == 0] * 6})
    frames, beams = [], []
    for k in range(stories):
        for j in range(ny):
            for i in range(nx):
                frames.append({"nodes": [index[(i, j, k)], index[(i, j, k + 1)]], "material": 0, "section": 0})
    for k in range(1, nz):
        for j in range(ny):
            for i in range(bx):
                beams.append(len(frames))
                frames.append({"nodes": [index[(i, j, k)], index[(i + 1, j, k)]], "material": 0, "section": 0})
        for j in range(by):
            for i in range(nx):
                beams.append(len(frames))
                frames.append({"nodes": [index[(i, j, k)], index[(i, j + 1, k)]], "material": 0, "section": 0})
    dead = {"name": "dead", "member": [
        {"member": b, "type": "distributed", "start": 0.0, "end": BAY,
         "start_load": [0.0, W_DEAD, 0.0], "end_load": [0.0, W_DEAD, 0.0], "axes": "global"}
        for b in beams]}
    wind = {"name": "wind", "nodal": [
        {"node": index[(i, j, k)], "force": [P_WIND, 0.0, 0.0]}
        for k in range(1, nz) for j in range(ny) for i in range(nx)]}
    combinations = [{"name": f"c{c}", "terms": [[0, 1.0 + 0.05 * c], [1, (-1.0) ** c * (0.5 + 0.1 * c)]]}
                    for c in range(combos)]
    model = {"schema_version": 1, "nodes": nodes,
             "materials": [{"young": E, "poisson": NU, "density": RHO}],
             "sections": [SECTION], "frames": frames,
             "load_cases": [dead, wind], "combinations": combinations}
    top = index[(bx, by, stories)]
    return model, top


def pynite_model(model):
    ref = FEModel3D()
    for i, n in enumerate(model["nodes"]):
        ref.add_node(f"n{i}", *n["position"])
        ref.def_support(f"n{i}", *n["restrained"])
    ref.add_material("m0", E, E / (2 * (1 + NU)), NU, RHO)
    s = SECTION
    ref.add_section("s0", s["area"], s["iy"], s["iz"], s["torsion"])
    for i, f in enumerate(model["frames"]):
        ref.add_member(f"f{i}", f"n{f['nodes'][0]}", f"n{f['nodes'][1]}", "m0", "s0")
    for load in model["load_cases"][0]["member"]:
        ref.add_member_dist_load(f"f{load['member']}", "FY", W_DEAD, W_DEAD, 0.0, BAY, "dead")
    for load in model["load_cases"][1]["nodal"]:
        ref.add_node_load(f"n{load['node']}", "FX", P_WIND, "wind")
    for c in model["combinations"]:
        ref.add_load_combo(c["name"], {"dead": c["terms"][0][1], "wind": c["terms"][1][1]})
    return ref


def time_oa(model, threads, in_flight):
    request = {"analysis": "static", "model": model,
               "options": {"threads": threads, "max_in_flight": in_flight,
                           "outputs": {"shells": False}}}
    t = time.perf_counter()
    result = json.loads(oa.solve_json(json.dumps(request)))
    return time.perf_counter() - t, result


def run_case(bx, by, stories, combos, run_pynite):
    model, top = frame_model(bx, by, stories, combos)
    row = {"size": f"{bx}x{by}x{stories}", "nodes": len(model["nodes"]),
           "members": len(model["frames"]), "dofs": 6 * len(model["nodes"]),
           "combos": combos}
    # Warm up the Rayon pool once so thread spawn is not billed to the first case.
    time_oa(model, 0, 16)
    model_json = json.dumps(model)
    t = time.perf_counter()
    oa.validate_model_json(model_json)
    row["oa_parse_prep_s"] = time.perf_counter() - t
    row["oa_serial_s"], serial = time_oa(model, 1, 1)
    row["oa_parallel_s"], parallel = time_oa(model, 0, 16)
    a = np.asarray([c["displacements"] for c in serial["combinations"]])
    b = np.asarray([c["displacements"] for c in parallel["combinations"]])
    # Multithreaded factorization changes summation order; agreement is to roundoff.
    assert np.abs(a - b).max() <= 1e-10 * np.abs(a).max(), "serial/parallel mismatch"
    row["top_dx_m"] = parallel["combinations"][0]["displacements"][top][0]
    if run_pynite:
        # Pynite's add_* calls are cheap; element preparation happens inside analyze.
        ref = pynite_model(model)
        t = time.perf_counter()
        ref.analyze_linear(log=False)
        row["pynite_analyze_s"] = time.perf_counter() - t
        expected = np.asarray([[[getattr(ref.nodes[f"n{i}"], d)[c["name"]] for d in ("DX", "DY", "DZ", "RX", "RY", "RZ")]
                                for i in range(len(model["nodes"]))] for c in model["combinations"]])
        scale = np.abs(expected).max()
        row["max_rel_diff"] = float(np.abs(a - expected).max() / scale)
        assert row["max_rel_diff"] < 1e-6, f"results disagree: {row['max_rel_diff']}"
        row["speedup_serial"] = row["pynite_analyze_s"] / row["oa_serial_s"]
        row["speedup_parallel"] = row["pynite_analyze_s"] / row["oa_parallel_s"]
    print(json.dumps(row), flush=True)
    return row


def table(rows):
    head = "| Frame | Nodes | DOFs | Combos | Pynite | OA 1 thread | OA 16 threads | of which parse + prep | Speedup 1T | Speedup 16T | Max rel diff |"
    out = [head, "|" + "---|" * 11]
    for r in rows:
        py = f"{r['pynite_analyze_s']:.2f} s" if "pynite_analyze_s" in r else "skipped"
        sp = lambda k: f"{r[k]:.0f}x" if k in r else "n/a"
        diff = f"{r['max_rel_diff']:.1e}" if "max_rel_diff" in r else "n/a"
        out.append(f"| {r['size']} | {r['nodes']:,} | {r['dofs']:,} | {r['combos']} | {py} | "
                   f"{r['oa_serial_s']:.3f} s | {r['oa_parallel_s']:.3f} s | {r['oa_parse_prep_s']:.3f} s | {sp('speedup_serial')} | {sp('speedup_parallel')} | {diff} |")
    return "\n".join(out)


sizes = [tuple(int(v) for v in s.split("x")) for s in args.sizes.split(",")]
size_rows, run_pynite = [], True
for bx, by, st in sizes:
    row = run_case(bx, by, st, args.combos, run_pynite)
    size_rows.append(row)
    if run_pynite and row.get("pynite_analyze_s", 0) > args.pynite_limit:
        run_pynite = False
sweep_rows = []
if args.combo_sweep:
    size, counts = args.combo_sweep.split(":")
    bx, by, st = (int(v) for v in size.split("x"))
    for combos in (int(c) for c in counts.split(",")):
        sweep_rows.append(run_case(bx, by, st, combos, True))

info = {"cpu": platform.processor(), "cores": os.cpu_count(), "python": platform.python_version(),
        "numpy": np.__version__, "pynite": __import__("Pynite").__version__, "oa": oa.__version__}
print("\n## Model size sweep\n")
print(table(size_rows))
print("\n## Combination count sweep\n")
print(table(sweep_rows))
print("\n" + json.dumps(info))
if args.json:
    args.json.write_text(json.dumps({"info": info, "sizes": size_rows, "combos": sweep_rows}, indent=2))
