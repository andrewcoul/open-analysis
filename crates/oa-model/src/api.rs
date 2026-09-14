//! JSON entry points shared by the WebAssembly binding and any other client
//! that speaks the command protocol.
use crate::{command::Command, compile, format, model::Model};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    Format(#[from] format::FormatError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Model(#[from] crate::command::ModelError),
    #[error("model has problems: {0}")]
    Problems(String),
}

/// Applies a JSON array of commands to a model document and returns the new document.
pub fn apply_commands_json(model: &str, commands: &str) -> Result<String, ApiError> {
    let mut model = format::from_json(model)?;
    let commands: Vec<Command> = serde_json::from_str(commands)?;
    Command::Batch { commands }.apply(&mut model)?;
    Ok(format::to_json(&model))
}

/// Compiles a model document to solver input plus the mapping, as JSON.
pub fn compile_json(model: &str) -> Result<String, ApiError> {
    let model: Model = format::from_json(model)?;
    let compiled = compile::compile(&model).map_err(|problems| {
        ApiError::Problems(
            problems
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    Ok(serde_json::to_string(&compiled)?)
}

/// Compiles and solves in one step, returning the solver's JSON response.
pub fn solve_json(model: &str, options: &str) -> Result<String, ApiError> {
    let model: Model = format::from_json(model)?;
    let compiled = compile::compile(&model).map_err(|problems| {
        ApiError::Problems(
            problems
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    let options: oa_core::StaticOptions = serde_json::from_str(options)?;
    let request = oa_core::AnalysisRequest::Static {
        model: compiled.solver,
        options,
    };
    oa_core::solve_json(&serde_json::to_string(&request)?)
        .map_err(|e| ApiError::Problems(e.to_string()))
}
