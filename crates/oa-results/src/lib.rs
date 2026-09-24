//! SQLite result store. One database per analysis run, written through the
//! solver's `ResultConsumer` and queried for envelopes, drifts, and raw SQL.
//! Every store records the content hash of the model it was computed from.
use oa_core::{
    CombinationResult, Envelope, FrameResult, Model, ResultConsumer, ShellResult, StaticOptions,
};
use rusqlite::{
    Connection, OptionalExtension,
    hooks::{AuthAction, AuthContext, Authorization},
    params, params_from_iter,
    types::ValueRef,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

/// Version 2 records which combinations a run was asked for and whether all
/// of them were stored, so a run that stopped early is never mistaken for a
/// finished one.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    Core(#[from] oa_core::Error),
    #[error("result store: {0}")]
    Store(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error(
        "result store is incomplete: the run never stored combinations {missing:?}; open it with open_partial to inspect what finished"
    )]
    Incomplete { missing: Vec<String> },
}
pub type Result<T> = std::result::Result<T, Error>;

/// Stable content hash of a solver model. See `Model::content_hash`.
pub fn model_hash(model: &Model) -> Result<String> {
    Ok(model.content_hash())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    pub schema_version: u32,
    pub model_hash: String,
    pub solver_version: String,
    pub created_unix: u64,
    /// The `StaticOptions` used, as JSON.
    pub options: String,
    /// Every combination the run was asked to compute, in request order.
    pub expected_combinations: Vec<String>,
    /// True once every expected combination has been committed.
    pub complete: bool,
}

/// A read-only query result for callers, such as agents, that write their own SQL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub truncated: bool,
}

pub const DISPLACEMENT_COLUMNS: [&str; 6] = ["ux", "uy", "uz", "rx", "ry", "rz"];
pub const FRAME_FORCE_COLUMNS: [&str; 12] = [
    "n_i", "vy_i", "vz_i", "t_i", "my_i", "mz_i", "n_j", "vy_j", "vz_j", "t_j", "my_j", "mz_j",
];
const FRAME_DISPLACEMENT_COLUMNS: [&str; 12] = [
    "ux_i", "uy_i", "uz_i", "rx_i", "ry_i", "rz_i", "ux_j", "uy_j", "uz_j", "rx_j", "ry_j", "rz_j",
];
const SHELL_STRESS_COLUMNS: [&str; 8] = ["sx", "sy", "sxy", "mx", "my", "mxy", "qx", "qy"];

fn schema() -> String {
    let node_table = |name: &str| {
        format!(
            "create table {name}(combination integer not null references combinations(id), node integer not null, {}, primary key(combination, node)) without rowid;\ncreate index {name}_node on {name}(node);",
            DISPLACEMENT_COLUMNS
                .iter()
                .map(|c| format!("{c} real not null"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let frame_columns = FRAME_FORCE_COLUMNS
        .iter()
        .chain(FRAME_DISPLACEMENT_COLUMNS.iter())
        .map(|c| format!("{c} real not null"))
        .collect::<Vec<_>>()
        .join(", ");
    let shell_columns = (1..=24)
        .map(|i| format!("f{i} real not null"))
        .chain(
            SHELL_STRESS_COLUMNS
                .iter()
                .map(|c| format!("{c} real not null")),
        )
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "create table run(id integer primary key check (id = 1), schema_version integer not null, model_hash text not null, solver_version text not null, created_unix integer not null, options text not null, expected_combinations text not null, complete integer not null default 0);
create table combinations(id integer primary key, name text not null unique, iterations integer not null, relative_residual real not null);
{}
{}
create table frame_results(combination integer not null references combinations(id), frame integer not null, active integer not null, {frame_columns}, primary key(combination, frame)) without rowid;
create index frame_results_frame on frame_results(frame);
create table shell_results(combination integer not null references combinations(id), shell integer not null, {shell_columns}, primary key(combination, shell)) without rowid;
create index shell_results_shell on shell_results(shell);",
        node_table("displacements"),
        node_table("reactions"),
    )
}

pub struct ResultStore {
    conn: Connection,
    info: RunInfo,
    combos: BTreeMap<String, i64>,
    /// Set by `open_partial`: the caller asked to read an unfinished run.
    allow_partial: bool,
    /// Stored tables a caller's view shadows; `sql` refuses to read them
    /// directly by `main.<table>` so the view is the only way in.
    shadowed: Vec<String>,
}

impl ResultStore {
    /// Creates a new store for one analysis run. `path` may be `:memory:`.
    /// The combinations the run will compute are recorded up front, and the
    /// run counts as complete only once every one of them has been written.
    pub fn create(path: impl AsRef<Path>, model: &Model, options: &StaticOptions) -> Result<Self> {
        let expected: Vec<String> = if options.combinations.is_empty() {
            model
                .effective_combinations()
                .into_iter()
                .map(|c| c.name)
                .collect()
        } else {
            options.combinations.clone()
        };
        if expected.is_empty() {
            return Err(Error::Store(
                "a run needs at least one combination to store".into(),
            ));
        }
        let mut unique = std::collections::HashSet::new();
        if let Some(duplicate) = expected.iter().find(|n| !unique.insert(n.as_str())) {
            return Err(Error::Store(format!(
                "combination {duplicate:?} is requested twice"
            )));
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("pragma journal_mode = wal; pragma synchronous = normal;")?;
        conn.execute_batch(&schema())?;
        let info = RunInfo {
            schema_version: SCHEMA_VERSION,
            model_hash: model_hash(model)?,
            solver_version: env!("CARGO_PKG_VERSION").into(),
            created_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            options: serde_json::to_string(options)?,
            expected_combinations: expected,
            complete: false,
        };
        conn.execute(
            "insert into run(id, schema_version, model_hash, solver_version, created_unix, options, expected_combinations, complete) values(1, ?1, ?2, ?3, ?4, ?5, ?6, 0)",
            params![
                info.schema_version,
                info.model_hash,
                info.solver_version,
                info.created_unix as i64,
                info.options,
                serde_json::to_string(&info.expected_combinations)?,
            ],
        )?;
        Ok(Self {
            conn,
            info,
            combos: BTreeMap::new(),
            allow_partial: false,
            shadowed: vec![],
        })
    }
    pub fn create_in_memory(model: &Model, options: &StaticOptions) -> Result<Self> {
        Self::create(":memory:", model, options)
    }
    /// Opens a finished store. A run that stopped before every requested
    /// combination was written is refused with [`Error::Incomplete`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let store = Self::open_partial(path)?;
        if !store.info.complete {
            return Err(Error::Incomplete {
                missing: store.missing_combinations(),
            });
        }
        Ok(store)
    }
    /// Opens a store whether or not its run finished. Check [`RunInfo::complete`]
    /// and [`ResultStore::missing_combinations`] before trusting envelopes,
    /// which only range over what was stored.
    pub fn open_partial(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        let (info, expected_json): (RunInfo, String) = conn
            .query_row(
                "select schema_version, model_hash, solver_version, created_unix, options, expected_combinations, complete from run where id = 1",
                [],
                |r| {
                    Ok((
                        RunInfo {
                            schema_version: r.get(0)?,
                            model_hash: r.get(1)?,
                            solver_version: r.get(2)?,
                            created_unix: r.get::<_, i64>(3)? as u64,
                            options: r.get(4)?,
                            expected_combinations: vec![],
                            complete: r.get::<_, i64>(6)? != 0,
                        },
                        r.get(5)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| Error::Store("no run record; not a result store".into()))?;
        if info.schema_version != SCHEMA_VERSION {
            return Err(Error::Store(format!(
                "schema version {} is not supported (expected {SCHEMA_VERSION})",
                info.schema_version
            )));
        }
        let mut info = info;
        info.expected_combinations = serde_json::from_str(&expected_json)?;
        let mut combos = BTreeMap::new();
        let mut stmt = conn.prepare("select id, name from combinations order by id")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (id, name) = row?;
            combos.insert(name, id);
        }
        drop(stmt);
        Ok(Self {
            conn,
            info,
            combos,
            allow_partial: true,
            shadowed: vec![],
        })
    }
    pub fn info(&self) -> &RunInfo {
        &self.info
    }
    /// Expected combinations that were never stored; empty once the run is complete.
    pub fn missing_combinations(&self) -> Vec<String> {
        self.info
            .expected_combinations
            .iter()
            .filter(|name| !self.combos.contains_key(*name))
            .cloned()
            .collect()
    }
    /// Reads are refused on an unfinished run unless the caller opened it
    /// with `open_partial`, so a partial envelope is never taken for a full one.
    fn require_complete(&self) -> Result<()> {
        if self.info.complete || self.allow_partial {
            Ok(())
        } else {
            Err(Error::Incomplete {
                missing: self.missing_combinations(),
            })
        }
    }
    /// True when the store was computed from exactly this model.
    pub fn matches(&self, model: &Model) -> Result<bool> {
        Ok(model_hash(model)? == self.info.model_hash)
    }
    pub fn combinations(&self) -> Vec<String> {
        let mut names: Vec<_> = self.combos.iter().collect();
        names.sort_by_key(|(_, id)| **id);
        names.into_iter().map(|(n, _)| n.clone()).collect()
    }
    /// Raw connection for callers that need their own SQL.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    fn write(&mut self, r: CombinationResult) -> Result<()> {
        if self.combos.contains_key(&r.combination) {
            return Err(Error::Store(format!(
                "combination {:?} already stored",
                r.combination
            )));
        }
        if !self.info.expected_combinations.contains(&r.combination) {
            return Err(Error::Store(format!(
                "combination {:?} was not part of this run",
                r.combination
            )));
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "insert into combinations(name, iterations, relative_residual) values(?1, ?2, ?3)",
            params![r.combination, r.iterations as i64, r.relative_residual],
        )?;
        let id = tx.last_insert_rowid();
        for (table, values) in [
            ("displacements", &r.displacements),
            ("reactions", &r.reactions),
        ] {
            if let Some(values) = values {
                let mut stmt = tx.prepare_cached(&format!(
                    "insert into {table}(combination, node, ux, uy, uz, rx, ry, rz) values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
                ))?;
                for (node, v) in values.iter().enumerate() {
                    stmt.execute(params![id, node as i64, v[0], v[1], v[2], v[3], v[4], v[5]])?;
                }
            }
        }
        if let Some(frames) = &r.frames {
            let placeholders = (1..=27)
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(", ");
            let mut stmt = tx.prepare_cached(&format!(
                "insert into frame_results(combination, frame, active, {}, {}) values({placeholders})",
                FRAME_FORCE_COLUMNS.join(", "),
                FRAME_DISPLACEMENT_COLUMNS.join(", ")
            ))?;
            for (frame, f) in frames.iter().enumerate() {
                let mut row: Vec<rusqlite::types::Value> = Vec::with_capacity(27);
                row.push((id).into());
                row.push((frame as i64).into());
                row.push((f.active as i64).into());
                row.extend(f.local_end_forces.iter().map(|v| (*v).into()));
                row.extend(f.local_displacements.iter().map(|v| (*v).into()));
                stmt.execute(params_from_iter(row))?;
            }
        }
        if let Some(shells) = &r.shells {
            let placeholders = (1..=34)
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(", ");
            let mut stmt = tx.prepare_cached(&format!(
                "insert into shell_results(combination, shell, {}, {}) values({placeholders})",
                (1..=24)
                    .map(|i| format!("f{i}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                SHELL_STRESS_COLUMNS.join(", ")
            ))?;
            for (shell, s) in shells.iter().enumerate() {
                let mut row: Vec<rusqlite::types::Value> = Vec::with_capacity(34);
                row.push(id.into());
                row.push((shell as i64).into());
                row.extend(s.local_end_forces.iter().map(|v| (*v).into()));
                row.extend(s.membrane_stress.iter().map(|v| (*v).into()));
                row.extend(s.bending_moment.iter().map(|v| (*v).into()));
                row.extend(s.transverse_shear.iter().map(|v| (*v).into()));
                stmt.execute(params_from_iter(row))?;
            }
        }
        // The last expected combination marks the run complete in the same
        // transaction, so the flag and the data land together or not at all.
        let finishing = self
            .info
            .expected_combinations
            .iter()
            .all(|name| *name == r.combination || self.combos.contains_key(name));
        if finishing {
            tx.execute("update run set complete = 1 where id = 1", [])?;
        }
        tx.commit()?;
        self.combos.insert(r.combination, id);
        self.info.complete = finishing;
        Ok(())
    }

    fn combo_id(&self, name: &str) -> Result<i64> {
        self.combos
            .get(name)
            .copied()
            .ok_or_else(|| Error::Store(format!("unknown combination {name:?}")))
    }
    /// Rehydrates one combination. Deselected outputs come back as `None`.
    pub fn combination(&self, name: &str) -> Result<CombinationResult> {
        self.require_complete()?;
        let id = self.combo_id(name)?;
        let (iterations, relative_residual): (i64, f64) = self.conn.query_row(
            "select iterations, relative_residual from combinations where id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let node_table = |table: &str| -> Result<Option<Vec<[f64; 6]>>> {
            let mut stmt = self.conn.prepare_cached(&format!(
                "select node, ux, uy, uz, rx, ry, rz from main.{table} where combination = ?1 order by node"
            ))?;
            let rows: Vec<[f64; 6]> = stmt
                .query_map([id], |r| {
                    Ok([
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ])
                })?
                .collect::<std::result::Result<_, _>>()?;
            Ok(if rows.is_empty() { None } else { Some(rows) })
        };
        let displacements = node_table("displacements")?;
        let reactions = node_table("reactions")?;
        let mut stmt = self.conn.prepare_cached(&format!(
            "select frame, active, {}, {} from main.frame_results where combination = ?1 order by frame",
            FRAME_FORCE_COLUMNS.join(", "),
            FRAME_DISPLACEMENT_COLUMNS.join(", ")
        ))?;
        let frames: Vec<FrameResult> = stmt
            .query_map([id], |r| {
                Ok(FrameResult {
                    active: r.get::<_, i64>(1)? != 0,
                    local_end_forces: std::array::from_fn(|i| r.get(2 + i).unwrap_or(f64::NAN)),
                    local_displacements: std::array::from_fn(|i| r.get(14 + i).unwrap_or(f64::NAN)),
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        let mut stmt = self.conn.prepare_cached(&format!(
            "select shell, {}, {} from main.shell_results where combination = ?1 order by shell",
            (1..=24)
                .map(|i| format!("f{i}"))
                .collect::<Vec<_>>()
                .join(", "),
            SHELL_STRESS_COLUMNS.join(", ")
        ))?;
        let shells: Vec<ShellResult> = stmt
            .query_map([id], |r| {
                Ok(ShellResult {
                    local_end_forces: std::array::from_fn(|i| r.get(1 + i).unwrap_or(f64::NAN)),
                    membrane_stress: std::array::from_fn(|i| r.get(25 + i).unwrap_or(f64::NAN)),
                    bending_moment: std::array::from_fn(|i| r.get(28 + i).unwrap_or(f64::NAN)),
                    transverse_shear: std::array::from_fn(|i| r.get(31 + i).unwrap_or(f64::NAN)),
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(CombinationResult {
            combination: name.into(),
            iterations: iterations as usize,
            relative_residual,
            displacements,
            reactions,
            frames: if frames.is_empty() {
                None
            } else {
                Some(frames)
            },
            shells: if shells.is_empty() {
                None
            } else {
                Some(shells)
            },
        })
    }
    /// One frame's result for a combination, for on-demand section forces.
    pub fn frame_result(&self, combination: &str, frame: usize) -> Result<Option<FrameResult>> {
        self.require_complete()?;
        let id = self.combo_id(combination)?;
        let mut stmt = self.conn.prepare_cached(&format!(
            "select active, {}, {} from main.frame_results where combination = ?1 and frame = ?2",
            FRAME_FORCE_COLUMNS.join(", "),
            FRAME_DISPLACEMENT_COLUMNS.join(", ")
        ))?;
        Ok(stmt
            .query_row(params![id, frame as i64], |r| {
                Ok(FrameResult {
                    active: r.get::<_, i64>(0)? != 0,
                    local_end_forces: std::array::from_fn(|i| r.get(1 + i).unwrap_or(f64::NAN)),
                    local_displacements: std::array::from_fn(|i| r.get(13 + i).unwrap_or(f64::NAN)),
                })
            })
            .optional()?)
    }

    fn envelope_column(
        &self,
        table: &str,
        key: &str,
        entity: usize,
        column: &str,
    ) -> Result<Envelope> {
        self.require_complete()?;
        let mut stmt = self.conn.prepare_cached(&format!(
            "select c.name, t.{column} from main.{table} t join combinations c on c.id = t.combination where t.{key} = ?1"
        ))?;
        let values: Vec<(String, f64)> = stmt
            .query_map([entity as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(Envelope::from_values(
            values.iter().map(|(n, v)| (n.as_str(), *v)),
        )?)
    }
    /// Envelope of one displacement component (0..6) at a node over all combinations.
    pub fn envelope_displacement(&self, node: usize, component: usize) -> Result<Envelope> {
        self.envelope_column(
            "displacements",
            "node",
            node,
            column(&DISPLACEMENT_COLUMNS, component)?,
        )
    }
    pub fn envelope_reaction(&self, node: usize, component: usize) -> Result<Envelope> {
        self.envelope_column(
            "reactions",
            "node",
            node,
            column(&DISPLACEMENT_COLUMNS, component)?,
        )
    }
    /// Envelope of one local end force (0..12) of a frame over all combinations.
    pub fn envelope_frame_force(&self, frame: usize, component: usize) -> Result<Envelope> {
        self.envelope_column(
            "frame_results",
            "frame",
            frame,
            column(&FRAME_FORCE_COLUMNS, component)?,
        )
    }
    /// Envelope of a displacement difference between two nodes, such as storey drift.
    pub fn envelope_drift(&self, upper: usize, lower: usize, component: usize) -> Result<Envelope> {
        self.require_complete()?;
        let col = column(&DISPLACEMENT_COLUMNS, component)?;
        let mut stmt = self.conn.prepare_cached(&format!(
            "select c.name, u.{col} - l.{col} from main.displacements u join main.displacements l on l.combination = u.combination join combinations c on c.id = u.combination where u.node = ?1 and l.node = ?2"
        ))?;
        let values: Vec<(String, f64)> = stmt
            .query_map(params![upper as i64, lower as i64], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(Envelope::from_values(
            values.iter().map(|(n, v)| (n.as_str(), *v)),
        )?)
    }
    /// Envelopes for a list of entities, which is how groups arrive from the model layer.
    pub fn envelope_displacements(
        &self,
        nodes: &[usize],
        component: usize,
    ) -> Result<Vec<(usize, Envelope)>> {
        nodes
            .iter()
            .map(|&n| Ok((n, self.envelope_displacement(n, component)?)))
            .collect()
    }
    pub fn envelope_frame_forces(
        &self,
        frames: &[usize],
        component: usize,
    ) -> Result<Vec<(usize, Envelope)>> {
        frames
            .iter()
            .map(|&f| Ok((f, self.envelope_frame_force(f, component)?)))
            .collect()
    }

    /// Runs a read-only SQL statement and returns at most `limit` rows.
    ///
    /// Defines a temporary view on this connection only; the file is not
    /// touched. A view named like a stored table shadows it for `sql`, which
    /// is how a caller presents the store in other units: the view's own
    /// query must then name the table as `main.<table>`, and `sql` refuses
    /// a statement that reads the shadowed table directly that way, so the
    /// view is the only route to it. The store's own reads always say
    /// `main.` so they see the stored values whatever views a caller has
    /// defined.
    pub fn define_view(&mut self, name: &str, select: &str) -> Result<()> {
        self.conn.execute_batch(&format!(
            "create temp view if not exists \"{name}\" as {select};"
        ))?;
        if !self.shadowed.iter().any(|t| t == name) {
            self.shadowed.push(name.to_string());
        }
        Ok(())
    }
    /// SQLite's own read-only classification lets `ATTACH`, `DETACH`, and
    /// pragmas through because they change no database content, yet they can
    /// create files and alter the connection. An authorizer restricts the
    /// statement to plain reads while it is prepared; the read-only check
    /// stays as a second guard.
    pub fn sql(&self, sql: &str, limit: usize) -> Result<Table> {
        self.require_complete()?;
        let shadowed = self.shadowed.clone();
        self.conn.authorizer(Some(move |ctx: AuthContext<'_>| {
            // Top-level SQL may not read a column of a shadowed table by
            // `main.<table>`; only the shadowing view (the `accessor`) may.
            // The column-less read SQLite reports for a flattened
            // `count(*)` names the table at top level but carries no value.
            if let AuthAction::Read {
                table_name,
                column_name,
            } = ctx.action
                && !column_name.is_empty()
                && ctx.database_name == Some("main")
                && ctx.accessor.is_none()
                && shadowed.iter().any(|t| t == table_name)
            {
                return Authorization::Deny;
            }
            read_only_authorizer(ctx)
        }))?;
        let prepared = self.conn.prepare(sql);
        self.conn
            .authorizer::<fn(AuthContext<'_>) -> Authorization>(None)?;
        let mut stmt = prepared?;
        if !stmt.readonly() {
            return Err(Error::Store("only read-only statements are allowed".into()));
        }
        let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let width = columns.len();
        let mut rows = vec![];
        let mut truncated = false;
        let mut query = stmt.query([])?;
        while let Some(row) = query.next()? {
            if rows.len() >= limit {
                truncated = true;
                break;
            }
            rows.push(
                (0..width)
                    .map(|i| match row.get_ref(i) {
                        Ok(ValueRef::Null) | Err(_) => serde_json::Value::Null,
                        Ok(ValueRef::Integer(v)) => serde_json::Value::from(v),
                        Ok(ValueRef::Real(v)) => serde_json::Number::from_f64(v)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null),
                        Ok(ValueRef::Text(t)) => {
                            serde_json::Value::String(String::from_utf8_lossy(t).into_owned())
                        }
                        Ok(ValueRef::Blob(b)) => {
                            serde_json::Value::String(format!("<{} bytes>", b.len()))
                        }
                    })
                    .collect(),
            );
        }
        Ok(Table {
            columns,
            rows,
            truncated,
        })
    }
}

/// Permits reading tables, columns, and functions and nothing else. Every
/// write, schema change, transaction control, `ATTACH`, `DETACH`, and pragma
/// makes preparation fail with a "not authorized" error.
fn read_only_authorizer(ctx: AuthContext<'_>) -> Authorization {
    match ctx.action {
        AuthAction::Select
        | AuthAction::Read { .. }
        | AuthAction::Function { .. }
        | AuthAction::Recursive => Authorization::Allow,
        _ => Authorization::Deny,
    }
}

fn column<'a>(names: &'a [&'a str], component: usize) -> Result<&'a str> {
    names
        .get(component)
        .copied()
        .ok_or_else(|| Error::Store(format!("component {component} out of range")))
}

impl ResultConsumer for ResultStore {
    fn consume(&mut self, result: CombinationResult) -> oa_core::Result<()> {
        self.write(result)
            .map_err(|e| oa_core::Error::Consumer(e.to_string()))
    }
}
