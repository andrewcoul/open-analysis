"""Differential verification against Pynite 3.2.0.

Run after `cargo build -p oa-py` (or installing the wheel).
Optional: --reference PATH for a Pynite checkout, --extension PATH for a raw
extension library, --write-fixtures to refresh the committed reference values.
The committed fixtures are generated from Pynite, never from open-analysis.
"""
import argparse
import copy
import importlib.machinery
import importlib.util
import json
import math
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--reference", type=Path)
parser.add_argument("--extension", type=Path)
parser.add_argument("--write-fixtures", action="store_true")
args = parser.parse_args()
if (ROOT / ".tools/python-deps").exists():
    sys.path.insert(0, str(ROOT / ".tools/python-deps"))
if args.reference:
    sys.path.insert(0, str(args.reference.resolve()))
import numpy as np
from Pynite import FEModel3D

if args.extension:
    extension = str(args.extension.resolve())
    loader = importlib.machinery.ExtensionFileLoader("_oa", extension)
    spec = importlib.util.spec_from_file_location("_oa", extension, loader=loader)
    oa = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(oa)
else:
    from open_analysis import _oa as oa


def reference_model(model):
    ref = FEModel3D()
    gravity = model.get("gravity", 9.80665)
    for i, node in enumerate(model["nodes"]):
        ref.add_node(f"n{i}", *node["position"])
        ref.def_support(f"n{i}", *node.get("restrained", [False] * 6))
        prescribed = node.get("prescribed", {})
        values = prescribed.get("translation", [0] * 3) + prescribed.get("rotation", [0] * 3)
        for dof, value in zip(["DX", "DY", "DZ", "RX", "RY", "RZ"], values):
            if value:
                ref.def_node_disp(f"n{i}", dof, value)
        springs = node.get("spring_translation", [0] * 3) + node.get("spring_rotation", [0] * 3)
        for dof, value in zip(["DX", "DY", "DZ", "RX", "RY", "RZ"], springs):
            if value:
                ref.def_support_spring(f"n{i}", dof, value)
    for i, m in enumerate(model["materials"]):
        # Pynite's rho is a weight density used only for self-weight.
        ref.add_material(f"m{i}", m["young"], m["young"] / (2 * (1 + m["poisson"])), m["poisson"], m.get("density", 0) * gravity)
    for i, s in enumerate(model.get("sections", [])):
        ref.add_section(f"s{i}", s["area"], s["iy"], s["iz"], s["torsion"])
    for i, f in enumerate(model.get("frames", [])):
        ref.add_member(f"f{i}", *(f"n{n}" for n in f["nodes"]), f"m{f['material']}", f"s{f['section']}", rotation=math.degrees(f.get("roll", 0)), tension_only=f.get("behavior") == "tension_only", comp_only=f.get("behavior") == "compression_only")
        ref.def_releases(f"f{i}", *f.get("releases", [False] * 12))
    for i, s in enumerate(model.get("shells", [])):
        method = ref.add_plate if s.get("formulation") == "rectangular" else ref.add_quad
        method(f"s{i}", *(f"n{n}" for n in s["nodes"]), s["thickness"], f"m{s['material']}")
    for case in model["load_cases"]:
        name = case["name"]
        for axis, factor in zip(["FX", "FY", "FZ"], case.get("self_weight", [0] * 3)):
            if factor:
                ref.add_member_self_weight(axis, factor, name)
        for load in case.get("nodal", []):
            values = load.get("force", [0] * 3) + load.get("moment", [0] * 3)
            for dof, value in zip(["FX", "FY", "FZ", "MX", "MY", "MZ"], values):
                if value:
                    ref.add_node_load(f"n{load['node']}", dof, value, name)
        for load in case.get("member", []):
            local = load.get("axes", "global") == "local"
            if load["type"] == "point":
                values = load.get("force", [0] * 3) + load.get("moment", [0] * 3)
                for dof, value in zip(["Fx", "Fy", "Fz", "Mx", "My", "Mz"], values):
                    if value:
                        ref.add_member_pt_load(f"f{load['member']}", dof if local else dof.upper(), value, load["position"], name)
            else:
                for d, dof in enumerate(["Fx", "Fy", "Fz"]):
                    a, b = load["start_load"][d], load["end_load"][d]
                    if a or b:
                        ref.add_member_dist_load(f"f{load['member']}", dof if local else dof.upper(), a, b, load["start"], load["end"], name)
        for load in case.get("surface", []):
            s = model["shells"][load["shell"]]
            method = ref.add_plate_surface_pressure if s.get("formulation") == "rectangular" else ref.add_quad_surface_pressure
            method(f"s{load['shell']}", load["pressure"], name)
    combos = model.get("combinations") or [{"name": c["name"], "terms": [[i, 1.0]]} for i, c in enumerate(model["load_cases"])]
    for c in combos:
        factors = {}
        for case, value in c["terms"]:
            name = model["load_cases"][case]["name"]
            factors[name] = factors.get(name, 0) + value
        ref.add_load_combo(c["name"], factors)
    return ref, combos


def basic(tip=(3, 0, 0)):
    return {"schema_version": 1, "nodes": [{"position": [0, 0, 0], "restrained": [True] * 6}, {"position": list(tip)}], "materials": [{"young": 200e9, "poisson": 0.3}], "sections": [{"area": 0.01, "iy": 2e-5, "iz": 4e-5, "torsion": 1e-5}], "frames": [{"nodes": [0, 1], "material": 0, "section": 0}], "load_cases": [{"name": "load"}]}


def cases():
    for i, tip in enumerate([(3, 0, 0), (0, 3, 0), (0, -3, 0), (0, 0, 3), (2, 3, -1)]):
        for roll in [0, 0.37]:
            m = basic(tip)
            m["frames"][0]["roll"] = roll
            m["load_cases"][0]["nodal"] = [{"node": 1, "force": [1234, -2345, 3456], "moment": [345, 456, -567]}]
            yield f"space_cantilever_{i}_roll_{roll}", m, "linear", 1e-8
    for axes in ["local", "global"]:
        for load_type in ["point", "distributed"]:
            m = basic((2, 3, -1))
            load = {"member": 0, "type": load_type, "axes": axes}
            if load_type == "point":
                load.update(position=1.13, force=[100, -200, 300], moment=[40, 50, -60])
            else:
                load.update(start=0.31, end=2.71, start_load=[100, -200, 300], end_load=[-70, -400, 10])
            m["load_cases"][0]["member"] = [load]
            yield f"sloped_partial_{axes}_{load_type}", m, "linear", 1e-8
    m = basic()
    m["nodes"][1]["restrained"] = [True] * 6
    m["frames"][0]["releases"] = [i in [4, 5, 10, 11] for i in range(12)]
    m["load_cases"][0]["member"] = [{"member": 0, "type": "distributed", "start": 0, "end": 3, "start_load": [0, -1000, 300], "end_load": [0, -1000, 300]}]
    yield "released_uniform_beam", m, "linear", 1e-8
    m = basic()
    m["nodes"][1]["restrained"] = [True] * 6
    m["nodes"][1]["prescribed"] = {"translation": [0.001, -0.002, 0.003], "rotation": [0.001, 0, 0]}
    yield "support_settlement", m, "linear", 1e-8
    m = basic((2, 3, -1))
    m["materials"][0]["density"] = 7850
    m["load_cases"][0]["self_weight"] = [0, -1, 0]
    m["load_cases"][0]["nodal"] = [{"node": 1, "force": [500, 0, 0]}]
    yield "self_weight_sloped", m, "linear", 1e-8
    m = basic()
    m["nodes"][1]["spring_translation"] = [0, 2e6, 3e5]
    m["nodes"][1]["spring_rotation"] = [0, 0, 4e5]
    m["load_cases"][0]["nodal"] = [{"node": 1, "force": [0, -1000, 700], "moment": [0, 0, 300]}]
    yield "tip_support_springs", m, "linear", 1e-8
    # Logan 5.30 geometry from Pynite's test_2D_frames.py, converted to SI.
    m = basic()
    m["nodes"] = [{"position": [x * 0.3048, y * 0.3048, 0], "restrained": [i in [0, 5]] * 6} for i, (x, y) in enumerate([(0, 0), (0, 30), (15, 40), (35, 40), (50, 30), (50, 0)])]
    m["materials"][0]["young"] = 30000 * 6894757.293168
    m["sections"][0] = {"area": 12 * 0.0254**2, "iy": 250 * 0.0254**4, "iz": 200 * 0.0254**4, "torsion": 250 * 0.0254**4}
    m["frames"] = [{"nodes": [i, i + 1], "material": 0, "section": 0} for i in range(5)]
    m["load_cases"][0]["nodal"] = [{"node": i, "force": [0, -30 * 4448.2216152605, 0]} for i in [2, 3]]
    yield "pynite_logan_5_30", m, "linear", 1e-8
    # Pynite test_TC_analysis.py geometry, explicit finite elements.
    m = basic()
    m["nodes"] = [{"position": [x * 0.0254, y * 0.0254, 0], "restrained": [False, False, True, True, True, True] if i == 1 else [True] * 6} for i, (x, y) in enumerate([(0, 0), (100, 0), (0, 10), (0, -10)])]
    m["materials"][0]["young"] = 29000 * 6894757.293168
    m["sections"][0] = {"area": 1.94 * 0.0254**2, "iy": 3 * 0.0254**4, "iz": 3 * 0.0254**4, "torsion": 0.0438 * 0.0254**4}
    m["frames"] = [{"nodes": [i, 1], "material": 0, "section": 0, "behavior": "both" if i == 0 else "tension_only", "releases": [j in [4, 5, 10, 11] for j in range(12)]} for i in [0, 2, 3]]
    m["load_cases"][0]["nodal"] = [{"node": 1, "force": [0, -10 * 4448.2216152605, 0]}]
    m["combinations"] = [{"name": "down", "terms": [[0, 1]]}, {"name": "up", "terms": [[0, -1]]}]
    yield "pynite_tension_only_braces", m, "nonlinear", 1e-7
    for form in ["rectangular", "dkmq"]:
        for distort in ([False] if form == "rectangular" else [False, True]):
            m = basic()
            m["frames"] = []
            m["nodes"] = []
            n = 4
            for j in range(n + 1):
                for i in range(n + 1):
                    x, y = i / n, j / n
                    if distort and 0 < i < n and 0 < j < n:
                        x += 0.04 * math.sin(i + j)
                        y += 0.03 * math.cos(i - j)
                    m["nodes"].append({"position": [x, y, 0], "restrained": [True, True, i in [0, n] or j in [0, n], False, False, True]})
            m["shells"] = []
            for j in range(n):
                for i in range(n):
                    a = j * (n + 1) + i
                    m["shells"].append({"nodes": [a, a + 1, a + n + 2, a + n + 1], "material": 0, "thickness": 0.02, "formulation": form})
            m["load_cases"][0]["surface"] = [{"shell": i, "pressure": -1000 * (1 + i / 20)} for i in range(len(m["shells"]))]
            yield f"{form}_plate_distorted_{distort}", m, "linear", 1e-7
    m = basic()
    n = 12
    m["nodes"] = [{"position": [3 * i / n, 0, 0], "restrained": [i == 0] * 6} for i in range(n + 1)]
    m["frames"] = [{"nodes": [i, i + 1], "material": 0, "section": 0} for i in range(n)]
    m["load_cases"][0]["nodal"] = [{"node": n, "force": [-300000, 1000, 0]}]
    # Pynite includes N/L in its axial geometric term; OA uses elastic axial
    # stiffness. Compare transverse responses independently below.
    yield "p_delta_column", m, "p_delta", 0.005


fixtures = []
for name, model, method, tolerance in cases():
    ref, combos = reference_model(model)
    if method == "p_delta":
        ref.analyze_PDelta(log=False)
    elif method == "nonlinear":
        ref.analyze(log=False)
    else:
        ref.analyze_linear(log=False)
    request = {"analysis": "static", "model": model, "options": {"method": method}}
    actual = json.loads(oa.solve_json(json.dumps(request)))["combinations"]
    expected = []
    for combo, result in zip(combos, actual, strict=True):
        cname = combo["name"]
        displacements = [[getattr(ref.nodes[f"n{i}"], dof)[cname] for dof in ["DX", "DY", "DZ", "RX", "RY", "RZ"]] for i in range(len(model["nodes"]))]
        reactions = [[getattr(ref.nodes[f"n{i}"], dof)[cname] for dof in ["RxnFX", "RxnFY", "RxnFZ", "RxnMX", "RxnMY", "RxnMZ"]] for i in range(len(model["nodes"]))]
        frame_forces = []
        for i in range(len(model.get("frames", []))):
            member = ref.members[f"f{i}"]
            active = member.active[cname]
            # Physical members contain exactly one finite element in these fixtures.
            forces = list(member.sub_members.values())[0].f(cname).reshape(-1).tolist() if active else [0.0] * 12
            frame_forces.append(forces)
        for field, wanted in [("displacements", displacements), ("reactions", reactions)]:
            a, b = np.asarray(result[field]), np.asarray(wanted)
            # Treat numerical zeros using a component-wise absolute floor.
            floor = 1e-10 if field == "displacements" else 1e-4
            np.testing.assert_allclose(a, b, rtol=tolerance, atol=floor, err_msg=f"{name}/{cname}/{field}")
        for i, forces in enumerate(frame_forces):
            np.testing.assert_allclose(result["frames"][i]["local_end_forces"], forces, rtol=tolerance, atol=1e-4, err_msg=f"{name}/{cname}/frame{i}")
        expected.append({"combination": cname, "displacements": displacements, "reactions": reactions, "frame_end_forces": frame_forces})
    fixtures.append({"name": name, "request": request, "relative_tolerance": tolerance, "expected": expected})
    print(f"PASS {name}", flush=True)

if args.write_fixtures:
    destination = ROOT / "crates/oa-core/tests/fixtures/pynite_reference.json"
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps({"reference": "Pynite 3.2.0 d8c822abd4dd9a3b0960a977ce7edd15d065aa76", "cases": fixtures}, indent=2) + "\n", encoding="utf-8")
print(f"Verified {len(fixtures)} models through the Python extension against Pynite.")
