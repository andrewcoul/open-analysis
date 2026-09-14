"""Run after installing the Python wheel with pip or maturin."""
import json
from pathlib import Path
from open_analysis import solve

request = json.loads(Path(__file__).with_name("cantilever.json").read_text())
static = solve(request)
print("Tip displacement, m:", static["combinations"][0]["displacements"][1][1])
modal = solve({"analysis": "modal", "model": request["model"], "options": {"modes": 3}})
print("Natural frequencies, Hz:", [m["frequency_hz"] for m in modal["result"]["modes"]])
