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
