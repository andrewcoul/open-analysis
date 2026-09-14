use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid model: {0}")]
    Model(String),
    #[error("unstable or singular stiffness matrix: {0}")]
    Unstable(String),
    #[error("numerical solver: {0}")]
    Solver(String),
    #[error(
        "combination {combination:?} did not converge after {iterations} iterations (relative residual {residual:e})"
    )]
    NonConvergence {
        combination: String,
        iterations: usize,
        residual: f64,
    },
    #[error("invalid analysis request: {0}")]
    Request(String),
    #[error("result consumer failed: {0}")]
    Consumer(String),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
