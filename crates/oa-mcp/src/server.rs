//! The MCP tools. Each is a thin wrapper that sends one job to a [`Host`],
//! which runs it on the session wherever that lives: behind a mutex for
//! the headless server, or on the GUI's thread when an agent attaches to
//! the desktop application.
use crate::{COMMAND_REFERENCE, Quantity, Session, SpectrumRequest};
use oa_model::{Command, EntityId, EntityKind, asce7};
use rmcp::{
    ErrorData as McpError, ServerHandler, handler::server::wrapper::Parameters, model::*, tool,
    tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// One tool call's work on the session.
pub type Job = Box<dyn FnOnce(&mut Session) -> crate::Result<Value> + Send>;
/// A job's outcome, with errors already put into words for the agent.
pub type Reply = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

/// Where the session lives and how a tool call reaches it.
pub trait Host: Send + Sync + 'static {
    fn run(&self, job: Job) -> Reply;
    /// Added to the server's instructions, for a host the agent should know
    /// about, such as a person watching in the GUI.
    fn instructions(&self) -> Option<&'static str> {
        None
    }
}

/// The headless server's host: a session of the agent's own.
impl Host for Mutex<Session> {
    fn run(&self, job: Job) -> Reply {
        let result = match self.lock() {
            Ok(mut session) => job(&mut session).map_err(|e| e.to_string()),
            Err(_) => Err("session poisoned".into()),
        };
        Box::pin(std::future::ready(result))
    }
}

#[derive(Clone)]
pub struct Server {
    host: Arc<dyn Host>,
    /// Tells this server's agent from others sharing the session, so each
    /// undoes only its own changes.
    agent: u64,
}

fn ok(value: Value) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )]))
}
fn fail(e: impl std::fmt::Display) -> McpError {
    McpError::invalid_params(e.to_string(), None)
}
fn kind(name: &str) -> Result<EntityKind, McpError> {
    serde_json::from_value(Value::String(name.into()))
        .map_err(|_| fail(format!("unknown entity kind {name:?}")))
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ListArgs {
    /// One of: level, node, material, section, frame, shell, diaphragm, load_case, combination, group, underlay, mass_source, grid_line.
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
    /// One of: level, node, material, section, frame, shell, diaphragm, load_case, combination, group, underlay, mass_source, grid_line.
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
    commands: Vec<Value>,
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
struct LibraryArgs {
    /// Case-insensitive substring of the section designation, such as "W14X".
    filter: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct DesignationArgs {
    /// Section designation, such as "W14X90"; case is ignored.
    designation: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct LibraryPickArgs {
    /// Designation in the library, such as "W14x90" or "A992".
    designation: String,
    /// Name the copy gets in the model.
    name: String,
}
#[derive(Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
enum Edition {
    #[serde(rename = "7-16")]
    Asce7_16,
    #[default]
    #[serde(rename = "7-22")]
    Asce7_22,
}
#[derive(Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DesignMethod {
    /// Section 2.3, LRFD.
    #[default]
    Strength,
    /// Section 2.4, ASD.
    AllowableStress,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct GenerateArgs {
    /// ASCE 7 edition, "7-16" or "7-22" (default).
    #[serde(default)]
    edition: Edition,
    /// "strength" (default) or "allowable_stress".
    #[serde(default)]
    method: DesignMethod,
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
    /// Multiplies every flexural stiffness modifier below 1 (frame iy and iz,
    /// shell membrane_x, membrane_y and bending), capped at 1, for this run
    /// only. 1.4 gives ACI 318 6.6.3.2.2 service-level stiffness for wind
    /// drift; leave it out for strength-level results.
    cracked_stiffness_factor: Option<f64>,
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
    /// Displacement component, usually "ux" or "uy" for lateral drift; Z is up.
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
#[derive(Deserialize, schemars::JsonSchema)]
struct ModalArgs {
    /// Number of modes, lowest first. Default 6.
    #[serde(default = "six")]
    modes: usize,
    /// The mass source to use, by name; the model's default when absent.
    #[serde(default)]
    mass_source: Option<String>,
}
fn six() -> usize {
    6
}
#[derive(Deserialize, schemars::JsonSchema)]
struct PeaksArgs {
    quantity: Quantity,
    /// Component name such as "ux" or "mz_i", or its index.
    component: String,
    /// One entity. Omit id and group to rank every node or frame.
    id: Option<u64>,
    /// A group whose members to rank.
    group: Option<u64>,
    #[serde(default = "twenty")]
    limit: usize,
}
fn twenty() -> usize {
    20
}

impl Server {
    pub fn new(host: Arc<dyn Host>) -> Self {
        static NEXT_AGENT: AtomicU64 = AtomicU64::new(1);
        Self {
            host,
            agent: NEXT_AGENT.fetch_add(1, Ordering::Relaxed),
        }
    }
    /// A server over a session of its own, for the headless binary.
    pub fn headless() -> Self {
        Self::new(Arc::new(Mutex::new(Session::default())))
    }
    async fn call(
        &self,
        job: impl FnOnce(&mut Session) -> crate::Result<Value> + Send + 'static,
    ) -> Result<CallToolResult, McpError> {
        let agent = self.agent;
        let job = move |s: &mut Session| s.as_agent(agent, job);
        ok(self.host.run(Box::new(job)).await.map_err(fail)?)
    }
}

#[tool_router]
impl Server {
    #[tool(description = "Sizes, names, and state of the open model. Call this first.")]
    async fn describe_model(&self) -> Result<CallToolResult, McpError> {
        self.call(|s| Ok(s.describe())).await
    }
    #[tool(
        description = "Compact rows for one entity kind, optionally filtered by name substring."
    )]
    async fn list_entities(
        &self,
        Parameters(a): Parameters<ListArgs>,
    ) -> Result<CallToolResult, McpError> {
        let kind = kind(&a.kind)?;
        self.call(move |s| Ok(s.list(kind, a.filter.as_deref(), a.limit)))
            .await
    }
    #[tool(description = "Full JSON of one entity by id.")]
    async fn get_entity(
        &self,
        Parameters(a): Parameters<IdArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| s.get(EntityId(a.id))).await
    }
    #[tool(description = "Id of the entity with this kind and exact name, or null.")]
    async fn find_entity(
        &self,
        Parameters(a): Parameters<FindArgs>,
    ) -> Result<CallToolResult, McpError> {
        let kind = kind(&a.kind)?;
        self.call(move |s| Ok(json!({"id": s.find(kind, &a.name)})))
            .await
    }
    #[tool(description = "Reserve fresh entity ids for add_* commands.")]
    async fn next_ids(
        &self,
        Parameters(a): Parameters<CountArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| Ok(json!({"ids": s.next_ids(a.count)})))
            .await
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
        self.call(move |s| s.apply(commands)).await
    }
    #[tool(
        description = "Undo your last applied command batch. Refused if the model was edited by anyone else since."
    )]
    async fn undo(&self) -> Result<CallToolResult, McpError> {
        self.call(|s| s.undo().map(|done| json!({"undone": done})))
            .await
    }
    #[tool(
        description = "Redo your last undone command batch. Refused if the model was edited by anyone else since."
    )]
    async fn redo(&self) -> Result<CallToolResult, McpError> {
        self.call(|s| s.redo().map(|done| json!({"redone": done})))
            .await
    }
    #[tool(description = "Start an empty model with this name, discarding the current one.")]
    async fn new_model(
        &self,
        Parameters(a): Parameters<NameArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| {
            s.new_model(&a.name)?;
            Ok(s.describe())
        })
        .await
    }
    #[tool(description = "Load a model document from a file path.")]
    async fn load_model(
        &self,
        Parameters(a): Parameters<PathArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| s.load(a.path.into())).await
    }
    #[tool(description = "Save the model document; path optional when it was loaded from a file.")]
    async fn save_model(
        &self,
        Parameters(a): Parameters<OptionalPathArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| s.save(a.path.map(Into::into)).map(|p| json!({"path": p})))
            .await
    }
    #[tool(
        description = "Section designations from the bundled AISC Shapes Database v16.0 (no single or double angles), filtered by substring and capped, and the bundled material designations."
    )]
    async fn library(
        &self,
        Parameters(a): Parameters<LibraryArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| Ok(s.library(a.filter.as_deref(), a.limit)))
            .await
    }
    #[tool(
        description = "One library section as add_section_from_library would copy it: area, second moments, shear areas, and AISC design properties (d, bf, tf, tw, Zx, Sx, rx, Cw, ...) with their units. x is the axis of iz."
    )]
    async fn library_section(
        &self,
        Parameters(a): Parameters<DesignationArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| s.library_section(&a.designation)).await
    }
    #[tool(description = "Copy a library section into the model under a name; returns its id.")]
    async fn add_section_from_library(
        &self,
        Parameters(a): Parameters<LibraryPickArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| {
            s.add_section_from_library(&a.designation, &a.name)
                .map(|id| json!({"id": id}))
        })
        .await
    }
    #[tool(description = "Copy a library material into the model under a name; returns its id.")]
    async fn add_material_from_library(
        &self,
        Parameters(a): Parameters<LibraryPickArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| {
            s.add_material_from_library(&a.designation, &a.name)
                .map(|id| json!({"id": id}))
        })
        .await
    }
    #[tool(
        description = "Add the ASCE 7 chapter 2 load combinations the load cases can form and the model lacks, as one undo step. Cases are matched by load_type, so set it on each case first."
    )]
    async fn generate_combinations(
        &self,
        Parameters(a): Parameters<GenerateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let edition = match a.edition {
            Edition::Asce7_16 => asce7::Edition::Asce7_16,
            Edition::Asce7_22 => asce7::Edition::Asce7_22,
        };
        let method = match a.method {
            DesignMethod::Strength => asce7::Method::Strength,
            DesignMethod::AllowableStress => asce7::Method::AllowableStress,
        };
        self.call(move |s| s.generate_combinations(edition, method))
            .await
    }
    #[tool(description = "Validate and compile the model. Problems name the entities involved.")]
    async fn compile(&self) -> Result<CallToolResult, McpError> {
        self.call(|s| s.compile()).await
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
            cracked_stiffness_factor: a.cracked_stiffness_factor.unwrap_or(1.0),
            ..Default::default()
        };
        self.call(move |s| s.analyze(options, a.store_path.map(Into::into)))
            .await
    }
    #[tool(
        description = "Envelope of one quantity at one entity over all combinations, with the governing combination."
    )]
    async fn envelope(
        &self,
        Parameters(a): Parameters<EnvelopeArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| s.envelope(EntityId(a.id), a.quantity, &a.component))
            .await
    }
    #[tool(description = "Envelopes for every member of a group, largest magnitude first.")]
    async fn group_envelope(
        &self,
        Parameters(a): Parameters<GroupEnvelopeArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| s.group_envelope(EntityId(a.group), a.quantity, &a.component, a.limit))
            .await
    }
    #[tool(
        description = "Storey drift: displacement of an upper node minus a lower node, with the ratio over their Z separation when it is not zero."
    )]
    async fn drift(
        &self,
        Parameters(a): Parameters<DriftArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| s.drift(EntityId(a.upper), EntityId(a.lower), &a.component))
            .await
    }
    #[tool(
        description = "Read-only SQL over the result store, in US customary units (in, rad, kip, kip·ft, ksi). Entity columns are solver indices; see entity_indices."
    )]
    async fn query_results(
        &self,
        Parameters(a): Parameters<QueryArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| s.query(&a.sql, a.limit)).await
    }
    #[tool(description = "Solver indices for entity ids, for use in query_results.")]
    async fn entity_indices(
        &self,
        Parameters(a): Parameters<IdsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let ids: Vec<EntityId> = a.ids.iter().map(|i| EntityId(*i)).collect();
        self.call(move |s| s.entity_indices(&ids)).await
    }
    #[tool(
        description = "Natural periods, frequencies, and mass participation of the lowest modes. Mass comes from the mass source named in mass_source, or the model's default one (describe_model lists them): node mass, the frames' and shells' own mass from material density unless turned off, and the downward load of the load cases the source lists. With no default source, it is node and element mass only; loads are not mass."
    )]
    async fn modal(
        &self,
        Parameters(a): Parameters<ModalArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| s.modal(a.modes, a.mass_source.as_deref()))
            .await
    }
    #[tool(
        description = "Response-spectrum analysis along one direction: modal summary, captured mass ratio, base reaction, and the largest peak displacements. Mass as for modal, from the source named in mass_source or the default. The spectrum is in g and must span every mode's period. Peaks are kept for spectrum_peaks until the next edit."
    )]
    async fn response_spectrum(
        &self,
        Parameters(request): Parameters<SpectrumRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| s.response_spectrum(&request)).await
    }
    #[tool(
        description = "Peaks from the last response_spectrum run, largest first, for one entity, a group's members, or every node or frame. They are unsigned magnitudes from a modal combination: never mix them with static envelopes, and do not subtract them for drift."
    )]
    async fn spectrum_peaks(
        &self,
        Parameters(a): Parameters<PeaksArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.call(move |s| {
            s.spectrum_peaks(
                a.quantity,
                &a.component,
                a.id.map(EntityId),
                a.group.map(EntityId),
                a.limit,
            )
        })
        .await
    }
}

/// A change in who is connected to [`serve_socket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    Opened,
    Closed,
}

/// Serves the tools over `host` to every agent that connects to `listener`,
/// each on its own task, until accepting fails. `on_connection` hears each
/// agent arrive and leave.
pub async fn serve_socket(
    mut listener: crate::socket::Listener,
    host: Arc<dyn Host>,
    on_connection: impl Fn(Connection) + Send + Sync + 'static,
) -> std::io::Result<()> {
    use rmcp::ServiceExt;
    let on_connection = Arc::new(on_connection);
    loop {
        let stream = listener.accept().await?;
        let server = Server::new(host.clone());
        let on_connection = on_connection.clone();
        tokio::spawn(async move {
            on_connection(Connection::Opened);
            if let Ok(service) = server.serve(stream).await {
                let _ = service.waiting().await;
            }
            on_connection(Connection::Closed);
        });
    }
}

const INSTRUCTIONS: &str = "Structural analysis. Build a model with apply_commands (see \
    command_reference), compile, analyze, then query envelopes, drifts, or SQL; modal and \
    response_spectrum cover dynamics. Names identify entities; ids come from next_ids. Units are \
    US customary (kip, ft, in; describe_model lists every symbol) on the way in and out. Z is up: \
    levels are planes of constant Z, every node binds to one, and a new model starts with Base \
    at 0.";

#[tool_handler]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        let instructions = match self.host.instructions() {
            Some(extra) => format!("{INSTRUCTIONS} {extra}"),
            None => INSTRUCTIONS.to_string(),
        };
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(instructions)
    }
}
