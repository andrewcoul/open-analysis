//! SQLite-backed pieces: the command journal for crash recovery, and the
//! hash check that keeps results attached only to the model they came from.
use crate::{command::Command, compile::Compiled};
use rusqlite::Connection;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("results were computed from a different model (hash {found}, expected {expected})")]
    StaleResults { expected: String, found: String },
    #[error("results are incomplete: the run never stored combinations {missing:?}")]
    IncompleteResults { missing: Vec<String> },
}

/// Append-only log of commands. Replaying it onto the model the session
/// started from reproduces the session.
pub struct Journal {
    conn: Connection,
}
impl Journal {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "pragma journal_mode = wal; create table if not exists commands(seq integer primary key, json text not null);",
        )?;
        Ok(Self { conn })
    }
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::open(":memory:")
    }
    pub fn append(&self, command: &Command) -> Result<(), StoreError> {
        self.conn.execute(
            "insert into commands(json) values(?1)",
            [serde_json::to_string(command)?],
        )?;
        Ok(())
    }
    pub fn replay(&self) -> Result<Vec<Command>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("select json from commands order by seq")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = vec![];
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }
    pub fn clear(&self) -> Result<(), StoreError> {
        self.conn.execute("delete from commands", [])?;
        Ok(())
    }
}

/// Refuses a result store that was not computed from exactly this compiled
/// model, or whose run stopped before every requested combination was stored.
pub fn attach(compiled: &Compiled, store: &oa_results::ResultStore) -> Result<(), StoreError> {
    if !store.info().complete {
        return Err(StoreError::IncompleteResults {
            missing: store.missing_combinations(),
        });
    }
    let expected = compiled.content_hash();
    let found = store.info().model_hash.clone();
    if expected != found {
        return Err(StoreError::StaleResults { expected, found });
    }
    Ok(())
}
