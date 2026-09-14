//! Versioned JSON protocol shared by CLI, Python and WebAssembly.
use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "analysis", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalysisRequest {
    Static {
        model: Model,
        #[serde(default)]
        options: StaticOptions,
    },
    Modal {
        model: Model,
        #[serde(default)]
        options: ModalOptions,
    },
    Spectrum {
        model: Model,
        options: crate::spectrum::SpectrumOptions,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "analysis", rename_all = "snake_case")]
pub enum AnalysisResponse {
    Static {
        schema_version: u32,
        combinations: Vec<CombinationResult>,
    },
    Modal {
        schema_version: u32,
        result: ModalResult,
    },
    Spectrum {
        schema_version: u32,
        result: crate::spectrum::SpectrumResult,
    },
}
pub fn solve(request: &AnalysisRequest) -> Result<AnalysisResponse> {
    match request {
        AnalysisRequest::Static { model, options } => Ok(AnalysisResponse::Static {
            schema_version: 1,
            combinations: analyze_static(model, options)?.combinations,
        }),
        AnalysisRequest::Modal { model, options } => Ok(AnalysisResponse::Modal {
            schema_version: 1,
            result: analyze_modal(model, options)?,
        }),
        AnalysisRequest::Spectrum { model, options } => Ok(AnalysisResponse::Spectrum {
            schema_version: 1,
            result: crate::spectrum::analyze_spectrum(model, options)?,
        }),
    }
}
pub fn solve_json(request: &str) -> Result<String> {
    serde_json::to_string(&solve(&serde_json::from_str(request)?)?).map_err(Error::from)
}
pub fn validate_model_json(json: &str) -> Result<()> {
    let model: Model = serde_json::from_str(json)?;
    crate::assembly::Prepared::new(&model)?;
    Ok(())
}
