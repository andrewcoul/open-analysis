use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
#[pyfunction]
fn solve_json(py: Python<'_>, request: String) -> PyResult<String> {
    py.detach(move || oa_core::solve_json(&request))
        .map_err(|e| PyValueError::new_err(e.to_string()))
}
#[pyfunction]
fn validate_model_json(py: Python<'_>, model: String) -> PyResult<()> {
    py.detach(move || oa_core::validate_model_json(&model))
        .map_err(|e| PyValueError::new_err(e.to_string()))
}
#[pymodule]
fn _oa(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_function(wrap_pyfunction!(solve_json, m)?)?;
    m.add_function(wrap_pyfunction!(validate_model_json, m)?)?;
    Ok(())
}
