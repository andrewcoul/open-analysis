//! Structural analysis with an immutable model, sparse assembly, and SI quantities.
pub mod analysis;
mod assembly;
mod element;
pub mod error;
pub mod io;
pub mod modal;
pub mod model;
pub mod results;
pub mod spectrum;
pub mod units;
pub use analysis::{
    OutputSelection, StaticMethod, StaticOptions, analyze_static, analyze_static_into,
};
pub use error::{Error, Result};
pub use io::{AnalysisRequest, AnalysisResponse, solve, solve_json, validate_model_json};
pub use modal::{ModalOptions, ModalResult, analyze_modal, modal_inertia_count};
pub use model::*;
pub use results::*;
pub use spectrum::{
    ModalCombination, SpectrumOptions, SpectrumPoint, SpectrumResult, analyze_spectrum,
};
