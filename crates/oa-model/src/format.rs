//! Versioned JSON file format with forward migrations, integrity checks on
//! load, and a save that never leaves the destination half written.
use crate::model::Model;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("format version {0} is newer than this build supports ({FORMAT_VERSION})")]
    TooNew(u32),
    #[error("missing or invalid format_version")]
    MissingVersion,
    #[error("corrupt document: {0}")]
    Corrupt(String),
}

pub fn to_json(model: &Model) -> String {
    serde_json::to_string_pretty(model).expect("model serializes")
}

/// Loads a model document, migrating older format versions forward and
/// checking the identity invariants every command relies on: ids are unique
/// across tables, group members exist, and the allocator is ahead of every id.
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
    let mut model: Model = serde_json::from_value(value)?;
    check_integrity(&mut model)?;
    Ok(model)
}

fn check_integrity(model: &mut Model) -> Result<(), FormatError> {
    let duplicates = model.duplicate_ids();
    if !duplicates.is_empty() {
        return Err(FormatError::Corrupt(format!(
            "ids used by more than one entity: {:?}",
            duplicates.iter().map(|d| d.0).collect::<Vec<_>>()
        )));
    }
    if let Some((group, member)) = model.dangling_group_members().first() {
        return Err(FormatError::Corrupt(format!(
            "group #{} lists missing entity #{}",
            group.0, member.0
        )));
    }
    let high_water = model.max_id().map_or(0, |id| id.0);
    if high_water == u64::MAX || model.next_id == u64::MAX {
        return Err(FormatError::Corrupt("entity id space is exhausted".into()));
    }
    // A stale allocator would hand out ids that are already taken. Move it
    // past the high-water mark; nothing else about the document changes.
    model.next_id = model.next_id.max(high_water + 1).max(1);
    Ok(())
}

/// Writes a model document without ever leaving the destination truncated.
/// The JSON goes to a sibling temporary file first, is flushed to disk, and
/// then replaces the destination in one rename. If any step fails the
/// previous document is untouched and the temporary file is removed.
pub fn save_json(model: &Model, path: &Path) -> std::io::Result<()> {
    let json = to_json(model);
    let temporary = temporary_path(path);
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// `model.json` is written through `model.json.tmp` beside it, so the final
/// rename stays within one directory and one filesystem.
pub fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "model".into());
    name.push(".tmp");
    path.with_file_name(name)
}

/// One step: a document at `from` becomes a document at `from + 1`.
/// Each version bump adds an arm here and a fixture under tests/fixtures.
fn migrate(from: u32, value: serde_json::Value) -> serde_json::Value {
    // Version 1 is the first format, so there is nothing to migrate yet.
    // The first bump turns this into `match from { 1 => ..., _ => value }`.
    debug_assert!(from < FORMAT_VERSION);
    value
}
