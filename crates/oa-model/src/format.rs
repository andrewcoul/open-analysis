//! Versioned JSON file format with forward migrations.
use crate::model::Model;

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("format version {0} is newer than this build supports ({FORMAT_VERSION})")]
    TooNew(u32),
    #[error("missing or invalid format_version")]
    MissingVersion,
}

pub fn to_json(model: &Model) -> String {
    serde_json::to_string_pretty(model).expect("model serializes")
}

/// Loads a model document, migrating older format versions forward.
pub fn from_json(text: &str) -> Result<Model, FormatError> {
    let mut value: serde_json::Value = serde_json::from_str(text)?;
    let mut version = value
        .get("format_version")
        .and_then(|v| v.as_u64())
        .ok_or(FormatError::MissingVersion)? as u32;
    if version > FORMAT_VERSION {
        return Err(FormatError::TooNew(version));
    }
    while version < FORMAT_VERSION {
        value = migrate(version, value);
        version += 1;
        value["format_version"] = serde_json::json!(version);
    }
    Ok(serde_json::from_value(value)?)
}

/// One step: a document at `from` becomes a document at `from + 1`.
/// Each version bump adds an arm here and a fixture under tests/fixtures.
fn migrate(from: u32, value: serde_json::Value) -> serde_json::Value {
    // Version 1 is the first format, so there is nothing to migrate yet.
    // The first bump turns this into `match from { 1 => ..., _ => value }`.
    debug_assert!(from < FORMAT_VERSION);
    value
}
