"""Structural analysis. All dictionary/JSON quantities are SI; IDs are zero based."""
import json
from ._oa import __version__, solve_json, validate_model_json

__all__ = ["__version__", "solve", "solve_json", "validate_model", "validate_model_json"]

def solve(request: dict) -> dict:
    """Run a static, modal, or spectrum request. Raises ValueError on failure.

    The Rust calculation releases Python's GIL. The input is not modified.
    """
    return json.loads(solve_json(json.dumps(request, allow_nan=False)))

def validate_model(model: dict) -> None:
    """Validate model properties and element geometry (does not prove stability)."""
    validate_model_json(json.dumps(model, allow_nan=False))
