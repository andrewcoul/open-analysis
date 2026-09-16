//! MCP server over stdio. Each tool is a thin wrapper around `oa_mcp::Session`.
use oa_mcp::{COMMAND_REFERENCE, Quantity, Session};
use oa_model::{Command, EntityId, EntityKind};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt, handler::server::wrapper::Parameters,
    model::*, tool, tool_handler, tool_router, transport::stdio,
};
use serde::Deserialize;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Server {
    session: Arc<Mutex<Session>>,
}

fn ok(value: serde_json::Value) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}
fn fail(e: impl std::fmt::Display) -> McpError {
    McpError::invalid_params(e.to_string(), None)
}
macro_rules! with_session {
    ($self:ident, |$s:ident| $body:expr) => {{
        let mut guard = $self.session.lock().map_err(|_| fail("session poisoned"))?;
        let $s = &mut *guard;
        let value = $body.map_err(fail)?;
        ok(value)
    }};
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ListArgs {
    /// One of: node, material, section, frame, shell, diaphragm, load_case, combination, group.
    kind: String,
    /// Substring of the name to match.
    filter: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    50
}
#[derive(Deserialize, schemars::JsonSchema)]
struct IdArgs {
    id: u64,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct FindArgs {
    /// One of: node, material, section, frame, shell, diaphragm, load_case, combination, group.
    kind: String,
    name: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct CountArgs {
    #[serde(default = "one")]
    count: usize,
}
fn one() -> usize {
    1
}
#[derive(Deserialize, schemars::JsonSchema)]
struct CommandsArgs {
    /// Commands as described by `command_reference`, applied atomically.
    commands: Vec<serde_json::Value>,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct NameArgs {
    name: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct PathArgs {
    path: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct OptionalPathArgs {
    path: Option<String>,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct LibraryPickArgs {
    /// Designation in the library, such as "W14x90" or "A992".
    designation: String,
    /// Name the copy gets in the model.
    name: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct AnalyzeArgs {
    /// "linear" (default), "nonlinear" for tension/compression-only members, or "p_delta".
    method: Option<String>,
    /// Combination names to run; empty runs all.
    #[serde(default)]
    combinations: Vec<String>,
    /// File to write the SQLite result store to; omitted keeps results in memory.
    store_path: Option<String>,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct EnvelopeArgs {
    id: u64,
    quantity: Quantity,
    /// Component name such as "ux" or "mz_i", or its index.
    component: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct GroupEnvelopeArgs {
    group: u64,
    quantity: Quantity,
    component: String,
    #[serde(default = "default_limit")]
    limit: usize,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct DriftArgs {
    upper: u64,
    lower: u64,
    /// Displacement component, usually "ux" or "uz" for lateral drift.
    component: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct QueryArgs {
    /// Read-only SQL over tables run, combinations, displacements, reactions, frame_results, shell_results.
    sql: String,
    #[serde(default = "default_limit")]
    limit: usize,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct IdsArgs {
    ids: Vec<u64>,
}

#[tool_router]
impl Server {
    fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(Session::default())),
        }
    }
    #[tool(description = "Sizes, names, and state of the open model. Call this first.")]
    async fn describe_model(&self) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| Ok::<_, McpError>(s.describe()))
    }
    #[tool(
        description = "Compact rows for one entity kind, optionally filtered by name substring."
    )]
    async fn list_entities(
        &self,
        Parameters(a): Parameters<ListArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| Ok::<_, McpError>(s.list(
            kind(&a.kind)?,
            a.filter.as_deref(),
            a.limit
        )))
    }
    #[tool(description = "Full JSON of one entity by id.")]
    async fn get_entity(
        &self,
        Parameters(a): Parameters<IdArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s.get(EntityId(a.id)))
    }
    #[tool(description = "Id of the entity with this kind and exact name, or null.")]
    async fn find_entity(
        &self,
        Parameters(a): Parameters<FindArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| Ok::<_, McpError>(
            serde_json::json!({"id": s.find(kind(&a.kind)?, &a.name)})
        ))
    }
    #[tool(description = "Reserve fresh entity ids for add_* commands.")]
    async fn next_ids(
        &self,
        Parameters(a): Parameters<CountArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| Ok::<_, McpError>(
            serde_json::json!({"ids": s.next_ids(a.count)})
        ))
    }
    #[tool(description = "Reference for every command accepted by apply_commands, with examples.")]
    async fn command_reference(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(
            COMMAND_REFERENCE,
        )]))
    }
    #[tool(
        description = "Apply a list of commands atomically; all succeed or none apply. Any edit discards results."
    )]
    async fn apply_commands(
        &self,
        Parameters(a): Parameters<CommandsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let commands: Vec<Command> = a
            .commands
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()
            .map_err(fail)?;
        with_session!(self, |s| s.apply(commands))
    }
    #[tool(description = "Undo the last applied command batch.")]
    async fn undo(&self) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s
            .undo()
            .map(|done| serde_json::json!({"undone": done})))
    }
    #[tool(description = "Redo the last undone command batch.")]
    async fn redo(&self) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s
            .redo()
            .map(|done| serde_json::json!({"redone": done})))
    }
    #[tool(description = "Start an empty model with this name, discarding the current one.")]
    async fn new_model(
        &self,
        Parameters(a): Parameters<NameArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| {
            s.new_model(&a.name);
            Ok::<_, McpError>(s.describe())
        })
    }
    #[tool(description = "Load a model document from a file path.")]
    async fn load_model(
        &self,
        Parameters(a): Parameters<PathArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s.load(a.path.into()))
    }
    #[tool(description = "Save the model document; path optional when it was loaded from a file.")]
    async fn save_model(
        &self,
        Parameters(a): Parameters<OptionalPathArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s
            .save(a.path.map(Into::into))
            .map(|p| serde_json::json!({"path": p})))
    }
    #[tool(description = "Section and material designations available in the bundled library.")]
    async fn library(&self) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| Ok::<_, McpError>(s.library()))
    }
    #[tool(description = "Copy a library section into the model under a name; returns its id.")]
    async fn add_section_from_library(
        &self,
        Parameters(a): Parameters<LibraryPickArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s
            .add_section_from_library(&a.designation, &a.name)
            .map(|id| serde_json::json!({"id": id})))
    }
    #[tool(description = "Copy a library material into the model under a name; returns its id.")]
    async fn add_material_from_library(
        &self,
        Parameters(a): Parameters<LibraryPickArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s
            .add_material_from_library(&a.designation, &a.name)
            .map(|id| serde_json::json!({"id": id})))
    }
    #[tool(description = "Validate and compile the model. Problems name the entities involved.")]
    async fn compile(&self) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s.compile())
    }
    #[tool(
        description = "Run a static analysis over the combinations and keep the results for queries."
    )]
    async fn analyze(
        &self,
        Parameters(a): Parameters<AnalyzeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let method = match a.method.as_deref() {
            None | Some("linear") => oa_core::StaticMethod::Linear,
            Some("nonlinear") => oa_core::StaticMethod::Nonlinear,
            Some("p_delta") => oa_core::StaticMethod::PDelta,
            Some(other) => return Err(fail(format!("unknown method {other:?}"))),
        };
        let options = oa_core::StaticOptions {
            method,
            combinations: a.combinations,
            ..Default::default()
        };
        with_session!(self, |s| s.analyze(options, a.store_path.map(Into::into)))
    }
    #[tool(
        description = "Envelope of one quantity at one entity over all combinations, with the governing combination."
    )]
    async fn envelope(
        &self,
        Parameters(a): Parameters<EnvelopeArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s.envelope(
            EntityId(a.id),
            a.quantity,
            &a.component
        ))
    }
    #[tool(description = "Envelopes for every member of a group, largest magnitude first.")]
    async fn group_envelope(
        &self,
        Parameters(a): Parameters<GroupEnvelopeArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s.group_envelope(
            EntityId(a.group),
            a.quantity,
            &a.component,
            a.limit
        ))
    }
    #[tool(
        description = "Storey drift: displacement of an upper node minus a lower node, with drift ratio when heights differ."
    )]
    async fn drift(
        &self,
        Parameters(a): Parameters<DriftArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s.drift(
            EntityId(a.upper),
            EntityId(a.lower),
            &a.component
        ))
    }
    #[tool(
        description = "Read-only SQL over the result store, in US customary units (in, rad, kip, kip·ft, ksi). Entity columns are solver indices; see entity_indices."
    )]
    async fn query_results(
        &self,
        Parameters(a): Parameters<QueryArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s.query(&a.sql, a.limit))
    }
    #[tool(description = "Solver indices for entity ids, for use in query_results.")]
    async fn entity_indices(
        &self,
        Parameters(a): Parameters<IdsArgs>,
    ) -> Result<CallToolResult, McpError> {
        with_session!(self, |s| s.entity_indices(
            &a.ids.iter().map(|i| EntityId(*i)).collect::<Vec<_>>()
        ))
    }
}

#[tool_handler]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Structural analysis. Build a model with apply_commands (see command_reference), \
                 compile, analyze, then query envelopes, drifts, or SQL. Names identify entities; \
                 ids come from next_ids. Units are US customary (kip, ft, in; describe_model \
                 lists every symbol) on the way in and out, and Y is up by convention.",
            )
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = Server::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn kind(name: &str) -> Result<EntityKind, McpError> {
    serde_json::from_value(serde_json::Value::String(name.into()))
        .map_err(|_| fail(format!("unknown entity kind {name:?}")))
}
