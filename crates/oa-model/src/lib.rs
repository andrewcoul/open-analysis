//! Editable structural model that compiles to the `oa-core` solver.
//!
//! Entities have stable ids and names, edits go through [`Command`]s that
//! return their inverse, and [`compile`] produces solver input plus a
//! two-way [`Mapping`]. See docs/model/PLAN.md.
pub mod api;
pub mod asce7;
pub mod command;
pub mod compile;
pub mod editor;
pub mod entity;
pub mod format;
pub mod library;
pub mod model;
#[cfg(feature = "store")]
pub mod store;
pub mod units;

pub use command::{Command, ModelError};
pub use compile::{Compiled, GroupIndices, Mapping, Problem, compile};
pub use editor::Editor;
pub use entity::*;
pub use format::{FORMAT_VERSION, from_json, save_json, to_json};
pub use library::Library;
pub use model::{EntityKind, Metadata, Model};
pub use units::{MapQuantities, Role, UnitSystem};
