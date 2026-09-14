use wasm_bindgen::prelude::*;
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").into()
}
#[wasm_bindgen]
pub fn solve_json(request: &str) -> Result<String, JsValue> {
    oa_core::solve_json(request).map_err(|e| JsValue::from_str(&e.to_string()))
}
#[wasm_bindgen]
pub fn validate_model_json(model: &str) -> Result<(), JsValue> {
    oa_core::validate_model_json(model).map_err(|e| JsValue::from_str(&e.to_string()))
}
/// Applies a JSON array of model-layer commands to a model document.
#[wasm_bindgen]
pub fn model_apply_commands(model: &str, commands: &str) -> Result<String, JsValue> {
    oa_model::api::apply_commands_json(model, commands)
        .map_err(|e| JsValue::from_str(&e.to_string()))
}
/// Compiles a model document to solver input plus the entity mapping.
#[wasm_bindgen]
pub fn model_compile(model: &str) -> Result<String, JsValue> {
    oa_model::api::compile_json(model).map_err(|e| JsValue::from_str(&e.to_string()))
}
/// Compiles and runs a static analysis on a model document.
#[wasm_bindgen]
pub fn model_solve(model: &str, options: &str) -> Result<String, JsValue> {
    oa_model::api::solve_json(model, options).map_err(|e| JsValue::from_str(&e.to_string()))
}
