//! Transport-independent session behind the MCP tools. Every tool is a
//! method here that takes plain arguments and returns JSON, so the whole
//! surface is testable without an agent or a transport.
//!
//! Every number an agent sends or receives is in [`UNITS`]. Commands are
//! converted to SI before they touch the model, entities are converted on
//! the way out, and the result store is read through views that rescale
//! each column, so raw SQL sees the same units as the envelope tools.
use oa_core::units::Acceleration;
use oa_core::{InMemoryResults, ResultConsumer, StaticOptions};
use oa_model::{compile::Compiled, *};
use oa_results::ResultStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

pub mod server;
pub mod socket;

/// The units of the whole agent surface.
pub const UNITS: UnitSystem = UnitSystem::UsCustomary;

fn display(role: Role, si: f64) -> f64 {
    UNITS.to_display(role, si)
}

/// The role of one component of a result quantity: translations then
/// rotations for displacements, forces then moments for reactions, and the
/// six-per-end force pattern for frames.
fn role_of(quantity: Quantity, component: usize) -> Role {
    match (quantity, component % 6 < 3) {
        (Quantity::Displacement, true) => Role::Displacement,
        (Quantity::Displacement, false) => Role::Rotation,
        (Quantity::Reaction | Quantity::FrameForce, true) => Role::Force,
        (Quantity::Reaction | Quantity::FrameForce, false) => Role::Moment,
    }
}

/// Views that shadow the stored tables and divide every column by its
/// unit, so `select ux from displacements` answers in inches. Entity and
/// combination columns pass through.
fn unit_views() -> Vec<(String, String)> {
    let f = |role: Role| UNITS.unit(role).si;
    let scaled = |names: &[&str], role: Role| -> Vec<String> {
        names
            .iter()
            .map(|c| format!("{c} / {} as {c}", f(role)))
            .collect()
    };
    let node = |table: &str, translation: Role, rotation: Role| {
        let mut cols = vec!["combination".to_string(), "node".to_string()];
        cols.extend(scaled(&["ux", "uy", "uz"], translation));
        cols.extend(scaled(&["rx", "ry", "rz"], rotation));
        (
            table.to_string(),
            format!("select {} from main.{table}", cols.join(", ")),
        )
    };
    let mut frame = vec!["combination".into(), "frame".into(), "active".into()];
    for end in ["i", "j"] {
        let forces: Vec<String> = ["n", "vy", "vz"]
            .iter()
            .map(|c| format!("{c}_{end}"))
            .collect();
        let moments: Vec<String> = ["t", "my", "mz"]
            .iter()
            .map(|c| format!("{c}_{end}"))
            .collect();
        let forces: Vec<&str> = forces.iter().map(String::as_str).collect();
        let moments: Vec<&str> = moments.iter().map(String::as_str).collect();
        frame.extend(scaled(&forces, Role::Force));
        frame.extend(scaled(&moments, Role::Moment));
    }
    for end in ["i", "j"] {
        let u: Vec<String> = ["ux", "uy", "uz"]
            .iter()
            .map(|c| format!("{c}_{end}"))
            .collect();
        let r: Vec<String> = ["rx", "ry", "rz"]
            .iter()
            .map(|c| format!("{c}_{end}"))
            .collect();
        let u: Vec<&str> = u.iter().map(String::as_str).collect();
        let r: Vec<&str> = r.iter().map(String::as_str).collect();
        frame.extend(scaled(&u, Role::Displacement));
        frame.extend(scaled(&r, Role::Rotation));
    }
    let mut shell = vec!["combination".to_string(), "shell".to_string()];
    for i in 1..=24 {
        let role = if (i - 1) % 6 < 3 {
            Role::Force
        } else {
            Role::Moment
        };
        shell.push(format!("f{i} / {} as f{i}", f(role)));
    }
    shell.extend(scaled(&["sx", "sy", "sxy"], Role::Stress));
    shell.extend(scaled(&["mx", "my", "mxy"], Role::MomentPerLength));
    shell.extend(scaled(&["qx", "qy"], Role::LineLoad));
    vec![
        node("displacements", Role::Displacement, Role::Rotation),
        node("reactions", Role::Force, Role::Moment),
        (
            "frame_results".into(),
            format!("select {} from main.frame_results", frame.join(", ")),
        ),
        (
            "shell_results".into(),
            format!("select {} from main.shell_results", shell.join(", ")),
        ),
    ]
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("{0}")]
    Model(#[from] ModelError),
    #[error("model has problems: {0}")]
    Problems(String),
    #[error("{0}")]
    Format(#[from] oa_model::format::FormatError),
    #[error("{0}")]
    Results(#[from] oa_results::Error),
    #[error("{0}")]
    Store(#[from] oa_model::store::StoreError),
    #[error("{0}")]
    Solver(#[from] oa_core::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
}
pub type Result<T> = std::result::Result<T, SessionError>;

/// One open model, its last compilation, and its current results.
///
/// The agent edits through the methods here. When a person edits the same
/// model, as in the GUI, their edits go through [`Session::user_apply`],
/// [`Session::user_undo`], and [`Session::user_redo`], so the session can
/// tell the two apart and keep an agent from undoing the person's work.
/// Several agents can share a session; [`Session::as_agent`] tells them
/// apart.
pub struct Session {
    editor: Editor,
    path: Option<PathBuf>,
    compiled: Option<Compiled>,
    store: Option<ResultStore>,
    spectrum: Option<oa_core::SpectrumResult>,
    /// Bumped by every change to the model, whoever makes it.
    revision: u64,
    /// The agent whose call is running; see [`Session::as_agent`].
    agent: u64,
    /// Who made each entry on the editor's undo and redo stacks, kept in
    /// step with them.
    undo_authors: Vec<Author>,
    redo_authors: Vec<Author>,
    /// Who made the last change of any kind.
    last_author: Option<Author>,
    /// The revision at the last load or save.
    saved_revision: u64,
    /// Bumped when the whole model is replaced by a new or loaded one.
    generation: u64,
    /// Refuse to replace a model that has unsaved changes. The GUI sets it,
    /// since a person's unsaved work cannot be undone back.
    pub protect_unsaved: bool,
    /// Keep a copy of every static result in memory beside the store. The
    /// GUI sets it to draw the results; the headless server does not, so
    /// its memory stays bounded.
    pub keep_in_memory: bool,
    fresh: Option<InMemoryResults>,
}
impl Default for Session {
    fn default() -> Self {
        Self::new(Model::default())
    }
}

/// Who made a change to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Author {
    User,
    Agent(u64),
}

/// Copies each combination into memory on its way to the store.
struct Tee<'a> {
    store: &'a mut ResultStore,
    copy: Option<&'a mut InMemoryResults>,
}
impl ResultConsumer for Tee<'_> {
    fn consume(&mut self, result: oa_core::CombinationResult) -> oa_core::Result<()> {
        if let Some(copy) = &mut self.copy {
            copy.consume(result.clone())?;
        }
        self.store.consume(result)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Quantity {
    /// Node displacement; components ux, uy, uz, rx, ry, rz.
    Displacement,
    /// Support or spring reaction; components as for displacement.
    Reaction,
    /// Frame end force in member axes; components n_i, vy_i, vz_i, t_i, my_i, mz_i, n_j, ...
    FrameForce,
}

fn component_index(quantity: Quantity, component: &str) -> Result<usize> {
    let names: &[&str] = match quantity {
        Quantity::Displacement | Quantity::Reaction => &oa_results::DISPLACEMENT_COLUMNS,
        Quantity::FrameForce => &oa_results::FRAME_FORCE_COLUMNS,
    };
    if let Ok(i) = component.parse::<usize>() {
        if i < names.len() {
            return Ok(i);
        }
    }
    names.iter().position(|n| *n == component).ok_or_else(|| {
        SessionError::Invalid(format!("component {component:?}; use one of {names:?}"))
    })
}

impl Session {
    pub fn new(model: Model) -> Self {
        Self {
            editor: Editor::new(model),
            path: None,
            compiled: None,
            store: None,
            spectrum: None,
            revision: 0,
            agent: 0,
            undo_authors: vec![],
            redo_authors: vec![],
            last_author: None,
            saved_revision: 0,
            generation: 0,
            protect_unsaved: false,
            keep_in_memory: false,
            fresh: None,
        }
    }
    /// A session over a model read from, or about to be saved to, `path`.
    pub fn with_path(model: Model, path: Option<PathBuf>) -> Self {
        Self {
            path,
            ..Self::new(model)
        }
    }
    pub fn model(&self) -> &Model {
        &self.editor.model
    }
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }
    pub fn can_undo(&self) -> bool {
        self.editor.can_undo()
    }
    pub fn can_redo(&self) -> bool {
        self.editor.can_redo()
    }
    /// Counts changes to the model, whoever makes them.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Counts replacements of the whole model by `new_model` or `load`.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Whether the model has changed since it was last loaded or saved.
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }
    /// Records that the model now matches the file at `path`.
    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.saved_revision = self.revision;
    }
    /// The static results of the last `analyze` when [`Session::keep_in_memory`]
    /// is set, with the compilation they belong to. Each run's results are
    /// handed out once.
    pub fn take_fresh_results(&mut self) -> Option<(Compiled, InMemoryResults)> {
        let results = self.fresh.take()?;
        Some((self.compiled.clone()?, results))
    }

    /// Runs `f` as agent `agent`, so the history knows its changes from
    /// other agents'. Each connection to the GUI is its own agent; calls
    /// made outside this are agent 0.
    pub fn as_agent<T>(&mut self, agent: u64, f: impl FnOnce(&mut Self) -> T) -> T {
        let outer = std::mem::replace(&mut self.agent, agent);
        let result = f(self);
        self.agent = outer;
        result
    }
    fn this_agent(&self) -> Author {
        Author::Agent(self.agent)
    }

    /// A change to the model: bumps the revision and drops everything
    /// derived from the old model.
    fn changed(&mut self, author: Author) {
        self.revision += 1;
        self.last_author = Some(author);
        self.compiled = None;
        self.store = None;
        self.spectrum = None;
        self.fresh = None;
    }
    /// Every edit, undo, and redo goes through these three, which keep the
    /// authors in step with the editor's stacks.
    fn edit(&mut self, author: Author, command: Command) -> std::result::Result<(), ModelError> {
        self.editor.apply(command)?;
        self.undo_authors.push(author);
        self.redo_authors.clear();
        self.changed(author);
        Ok(())
    }
    fn undo_as(&mut self, author: Author) -> std::result::Result<bool, ModelError> {
        let done = self.editor.undo()?;
        if done {
            self.redo_authors.extend(self.undo_authors.pop());
            self.changed(author);
        }
        Ok(done)
    }
    fn redo_as(&mut self, author: Author) -> std::result::Result<bool, ModelError> {
        let done = self.editor.redo()?;
        if done {
            self.undo_authors.extend(self.redo_authors.pop());
            self.changed(author);
        }
        Ok(done)
    }
    /// Refuses unless nobody else has changed the model since this agent
    /// last did, and the step `what` would act on, if any, is this agent's
    /// own, so an agent's undo and redo only ever take back its own work.
    fn check_history_is_agents(&self, what: &str, next: Option<&Author>) -> Result<()> {
        let me = self.this_agent();
        let mine = |author: Option<&Author>| author.is_none_or(|a| *a == me);
        if mine(self.last_author.as_ref()) && mine(next) {
            return Ok(());
        }
        Err(SessionError::Invalid(format!(
            "the step {what} would act on is not yours, or the user or another agent has edited \
             since your last change; ask the user to use Ctrl+Z or Ctrl+Y in the GUI, or make a \
             new change that reverses yours"
        )))
    }
    /// Replaces the whole model, refusing when that would discard a person's
    /// unsaved work.
    fn replace_model(&mut self, model: Model, path: Option<PathBuf>) -> Result<()> {
        if self.protect_unsaved && self.is_dirty() {
            return Err(SessionError::Invalid(
                "the model open in the GUI has unsaved changes; ask the user to save or discard \
                 them before a new model replaces it"
                    .into(),
            ));
        }
        let replacement = Self {
            revision: self.revision + 1,
            agent: self.agent,
            saved_revision: self.revision + 1,
            generation: self.generation + 1,
            protect_unsaved: self.protect_unsaved,
            keep_in_memory: self.keep_in_memory,
            ..Self::with_path(model, path)
        };
        *self = replacement;
        Ok(())
    }

    /// Applies a command, in SI, on behalf of the person rather than the agent.
    pub fn user_apply(&mut self, command: Command) -> std::result::Result<(), ModelError> {
        self.edit(Author::User, command)
    }
    pub fn user_undo(&mut self) -> std::result::Result<bool, ModelError> {
        self.undo_as(Author::User)
    }
    pub fn user_redo(&mut self) -> std::result::Result<bool, ModelError> {
        self.redo_as(Author::User)
    }

    /// Sizes and names an agent needs before it can do anything else.
    pub fn describe(&self) -> Value {
        let m = self.model();
        let names = |it: Vec<&str>| Value::from(it);
        let symbols: serde_json::Map<String, Value> = UNITS
            .symbols()
            .into_iter()
            .map(|(role, symbol)| {
                let key = serde_json::to_value(role)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                (key, json!(symbol))
            })
            .collect();
        json!({
            "name": m.metadata.name,
            "path": self.path,
            "units": {"system": UNITS.name(), "symbols": symbols},
            "gravity": display(Role::Acceleration, m.gravity.si()),
            "mass_sources": m.mass_sources.iter().map(|(id, s)| self.mass_source_row(*id, s)).collect::<Vec<_>>(),
            "default_mass_source": m.default_mass_source.and_then(|id| m.name_of(id)),
            "counts": {
                "levels": m.levels.len(),
                "nodes": m.nodes.len(), "materials": m.materials.len(), "sections": m.sections.len(),
                "frames": m.frames.len(), "shells": m.shells.len(), "diaphragms": m.diaphragms.len(),
                "load_cases": m.load_cases.len(), "combinations": m.combinations.len(), "groups": m.groups.len(),
                "grid_lines": m.grid_lines.len(),
            },
            "levels": m.levels_by_elevation().into_iter().map(|id| self.level_row(id)).collect::<Vec<_>>(),
            "load_cases": names(m.load_cases.values().map(|c| c.name.as_str()).collect()),
            "combinations": names(m.combinations.values().map(|c| c.name.as_str()).collect()),
            "groups": m.groups.iter().map(|(id, g)| json!({"id": id, "name": g.name, "size": g.members.len()})).collect::<Vec<_>>(),
            "grid_lines": names(m.grid_lines.values().map(|g| g.name.as_str()).collect()),
            "unsaved_changes": self.is_dirty(),
            "compiled": self.compiled.is_some(),
            "results": self.store.as_ref().map(|s| json!({"combinations": s.combinations(), "current": true})),
            "spectrum_results": self.spectrum.is_some(),
            "can_undo": self.editor.can_undo(),
            "can_redo": self.editor.can_redo(),
        })
    }
    /// A mass source with its cases by name, as describe_model and
    /// list_entities show it.
    fn mass_source_row(&self, id: EntityId, s: &MassSource) -> Value {
        let m = self.model();
        json!({
            "id": id, "name": s.name, "default": m.default_mass_source == Some(id),
            "element_mass": s.element_mass, "lateral": s.lateral, "vertical": s.vertical,
            "lump_to_levels": s.lump_to_levels,
            "cases": s.cases.iter().map(|(c, f)| json!({
                "id": c, "name": m.name_of(*c), "multiplier": f,
            })).collect::<Vec<_>>(),
        })
    }
    /// The compiled solver model with the named mass source, or with the
    /// default one when no name is given.
    fn solver_with(
        &mut self,
        mass_source: Option<&str>,
    ) -> Result<std::borrow::Cow<'_, oa_core::Model>> {
        let find = |name: &str| {
            self.model()
                .find::<MassSource>(name)
                .ok_or_else(|| SessionError::Invalid(format!("no mass source named {name:?}")))
        };
        let source = mass_source.map(find).transpose()?;
        self.compiled()?;
        let compiled = self.compiled.as_ref().expect("compiled above");
        Ok(match source {
            None => std::borrow::Cow::Borrowed(&compiled.solver),
            Some(id) => std::borrow::Cow::Owned(
                compiled
                    .with_mass_source(&self.editor.model, id)
                    .expect("compilation checked every mass source"),
            ),
        })
    }
    /// A level with its elevation and the storey height below it, in feet.
    fn level_row(&self, id: EntityId) -> Value {
        let m = self.model();
        let l = &m.levels[&id];
        json!({
            "id": id, "name": l.name,
            "elevation": display(Role::Length, l.elevation.si()),
            "height_below": oa_model::levels::height_below(m, id).map(|h| display(Role::Length, h)),
        })
    }
    /// Compact rows for one entity kind, optionally filtered by a name
    /// substring. Levels come lowest first.
    pub fn list(&self, kind: EntityKind, filter: Option<&str>, limit: usize) -> Value {
        let m = self.model();
        let matches = |name: &str| filter.is_none_or(|f| name.contains(f));
        let mut rows = vec![];
        let mut total = 0;
        if kind == EntityKind::Level {
            for id in m.levels_by_elevation() {
                if !matches(&m.levels[&id].name) {
                    continue;
                }
                total += 1;
                if rows.len() < limit {
                    rows.push(self.level_row(id));
                }
            }
            return json!({"total": total, "rows": rows, "truncated": total > rows.len()});
        }
        macro_rules! rows {
            ($table:expr, |$id:ident, $e:ident| $summary:expr) => {
                for ($id, $e) in $table.iter().filter(|(_, e)| matches(&e.name)) {
                    total += 1;
                    if rows.len() < limit {
                        rows.push($summary);
                    }
                }
            };
        }
        match kind {
            EntityKind::Level => unreachable!("listed above"),
            EntityKind::Node => rows!(
                m.nodes,
                |id, n| json!({
                    "id": id, "name": n.name,
                    "position": n.position.map(|p| display(Role::Length, p.si())),
                    "level": m.name_of(n.level),
                    "offset": m.levels.get(&n.level).map(|l| display(Role::Length, oa_model::levels::offset(n, l))),
                    "restrained": n.restrained,
                })
            ),
            EntityKind::Material => rows!(
                m.materials,
                |id, e| json!({"id": id, "name": e.name, "young": display(Role::Stress, e.young.si())})
            ),
            EntityKind::Section => rows!(
                m.sections,
                |id, e| json!({"id": id, "name": e.name, "area": display(Role::Area, e.area.si()), "provenance": e.provenance.as_ref().map(|p| &p.designation)})
            ),
            EntityKind::Frame => rows!(
                m.frames,
                |id, e| json!({"id": id, "name": e.name, "nodes": e.nodes, "section": e.section})
            ),
            EntityKind::Shell => rows!(
                m.shells,
                |id, e| json!({"id": id, "name": e.name, "nodes": e.nodes})
            ),
            EntityKind::Diaphragm => rows!(
                m.diaphragms,
                |id, e| json!({"id": id, "name": e.name, "nodes": e.nodes.len(), "master": e.master})
            ),
            EntityKind::LoadCase => rows!(
                m.load_cases,
                |id, e| json!({"id": id, "name": e.name, "load_type": e.load_type, "nodal": e.nodal.len(), "member": e.member.len(), "self_weight": e.self_weight})
            ),
            EntityKind::Combination => rows!(
                m.combinations,
                |id, e| json!({"id": id, "name": e.name, "terms": e.terms})
            ),
            EntityKind::Group => rows!(
                m.groups,
                |id, e| json!({"id": id, "name": e.name, "size": e.members.len()})
            ),
            EntityKind::Underlay => rows!(
                m.underlays,
                |id, e| json!({"id": id, "name": e.name, "level": m.name_of(e.level), "segments": e.segments.len()})
            ),
            EntityKind::MassSource => rows!(m.mass_sources, |id, e| self.mass_source_row(*id, e)),
            EntityKind::GridLine => rows!(
                m.grid_lines,
                |id, e| json!({"id": id, "name": e.name, "start": e.start.map(|v| display(Role::Length, v.si())), "end": e.end.map(|v| display(Role::Length, v.si()))})
            ),
        }
        json!({"total": total, "rows": rows, "truncated": total > rows.len()})
    }
    pub fn get(&self, id: EntityId) -> Result<Value> {
        let m = self.model();
        let kind = m.kind_of(id).ok_or(ModelError::NotFound(id))?;
        let value = match kind {
            EntityKind::Level => serde_json::to_value(UNITS.display(&m.levels[&id]))?,
            EntityKind::Node => serde_json::to_value(UNITS.display(&m.nodes[&id]))?,
            EntityKind::Material => serde_json::to_value(UNITS.display(&m.materials[&id]))?,
            EntityKind::Section => serde_json::to_value(UNITS.display(&m.sections[&id]))?,
            EntityKind::Frame => serde_json::to_value(UNITS.display(&m.frames[&id]))?,
            EntityKind::Shell => serde_json::to_value(UNITS.display(&m.shells[&id]))?,
            EntityKind::Diaphragm => serde_json::to_value(&m.diaphragms[&id])?,
            EntityKind::LoadCase => serde_json::to_value(UNITS.display(&m.load_cases[&id]))?,
            EntityKind::Combination => serde_json::to_value(&m.combinations[&id])?,
            EntityKind::Group => serde_json::to_value(&m.groups[&id])?,
            EntityKind::Underlay => serde_json::to_value(UNITS.display(&m.underlays[&id]))?,
            EntityKind::MassSource => serde_json::to_value(&m.mass_sources[&id])?,
            EntityKind::GridLine => serde_json::to_value(UNITS.display(&m.grid_lines[&id]))?,
        };
        Ok(json!({"id": id, "kind": kind, "entity": value}))
    }
    pub fn find(&self, kind: EntityKind, name: &str) -> Option<EntityId> {
        let m = self.model();
        match kind {
            EntityKind::Level => m.find::<Level>(name),
            EntityKind::Node => m.find::<Node>(name),
            EntityKind::Material => m.find::<Material>(name),
            EntityKind::Section => m.find::<Section>(name),
            EntityKind::Frame => m.find::<Frame>(name),
            EntityKind::Shell => m.find::<Shell>(name),
            EntityKind::Diaphragm => m.find::<Diaphragm>(name),
            EntityKind::LoadCase => m.find::<LoadCase>(name),
            EntityKind::Combination => m.find::<Combination>(name),
            EntityKind::Group => m.find::<Group>(name),
            EntityKind::Underlay => m.find::<Underlay>(name),
            EntityKind::MassSource => m.find::<MassSource>(name),
            EntityKind::GridLine => m.find::<GridLine>(name),
        }
    }
    /// Fresh ids for commands that add entities.
    pub fn next_ids(&mut self, count: usize) -> Vec<EntityId> {
        (0..count.clamp(1, 1000))
            .map(|_| self.editor.model.allocate())
            .collect()
    }
    /// Applies commands, given in [`UNITS`], atomically. Any change
    /// invalidates compilation and results.
    pub fn apply(&mut self, mut commands: Vec<Command>) -> Result<Value> {
        let count = commands.len();
        for command in &mut commands {
            command.map_quantities(&mut |role, v| UNITS.from_display(role, v));
        }
        self.edit(self.this_agent(), Command::Batch { commands })?;
        Ok(json!({"applied": count, "counts": self.describe()["counts"]}))
    }
    /// Undoes the agent's last batch. Refused when the batch on top of the
    /// stack is someone else's, or someone else has edited since.
    pub fn undo(&mut self) -> Result<bool> {
        self.check_history_is_agents("undo", self.undo_authors.last())?;
        Ok(self.undo_as(self.this_agent())?)
    }
    pub fn redo(&mut self) -> Result<bool> {
        self.check_history_is_agents("redo", self.redo_authors.last())?;
        Ok(self.redo_as(self.this_agent())?)
    }
    pub fn new_model(&mut self, name: &str) -> Result<()> {
        let mut model = Model::default();
        model.metadata.name = name.into();
        self.replace_model(model, None)
    }
    pub fn load(&mut self, path: PathBuf) -> Result<Value> {
        let model = from_json(&std::fs::read_to_string(&path)?)?;
        self.replace_model(model, Some(path))?;
        Ok(self.describe())
    }
    pub fn save(&mut self, path: Option<PathBuf>) -> Result<PathBuf> {
        let path = path
            .or_else(|| self.path.clone())
            .ok_or_else(|| SessionError::Invalid("no path given and the model has none".into()))?;
        oa_model::save_json(self.model(), &path)?;
        self.mark_saved(path.clone());
        Ok(path)
    }
    /// Section designations from the AISC shape table, optionally filtered
    /// by a case-insensitive substring and capped at `limit`, and the
    /// starter library's materials.
    pub fn library(&self, filter: Option<&str>, limit: usize) -> Value {
        let sections = Library::aisc();
        let materials = Library::starter();
        let filter = filter.map(str::to_ascii_uppercase);
        let matching: Vec<&str> = sections
            .section_designations()
            .into_iter()
            .filter(|d| {
                filter
                    .as_ref()
                    .is_none_or(|f| d.to_ascii_uppercase().contains(f))
            })
            .collect();
        json!({
            "sections": {
                "library": sections.name, "version": sections.version, "note": sections.note,
                "total": matching.len(),
                "designations": matching.iter().take(limit).collect::<Vec<_>>(),
                "truncated": matching.len() > limit,
            },
            "materials": {
                "library": materials.name, "version": materials.version, "note": materials.note,
                "designations": materials.materials.iter().map(|m| m.designation.as_str()).collect::<Vec<_>>(),
            },
        })
    }
    /// One library section as it would be copied into the model, in
    /// display units, with the unit of each design property.
    pub fn library_section(&self, designation: &str) -> Result<Value> {
        let section = Library::aisc()
            .section(designation, designation)
            .ok_or_else(|| {
                SessionError::Invalid(format!("no section {designation:?} in the library"))
            })?;
        let units: std::collections::BTreeMap<_, _> = section
            .shape
            .iter()
            .flat_map(|shape| shape.properties.keys())
            .map(|p| (*p, p.role().map_or("", |role| UNITS.symbol(role))))
            .collect();
        Ok(json!({"section": UNITS.display(&section), "property_units": units}))
    }
    pub fn add_section_from_library(&mut self, designation: &str, name: &str) -> Result<EntityId> {
        let section = Library::aisc().section(designation, name).ok_or_else(|| {
            SessionError::Invalid(format!("no section {designation:?} in the library"))
        })?;
        let id = self.editor.model.allocate();
        self.edit(self.this_agent(), Command::AddSection { id, section })?;
        Ok(id)
    }
    pub fn add_material_from_library(&mut self, designation: &str, name: &str) -> Result<EntityId> {
        let material = Library::starter()
            .material(designation, name)
            .ok_or_else(|| {
                SessionError::Invalid(format!("no material {designation:?} in the library"))
            })?;
        let id = self.editor.model.allocate();
        self.edit(self.this_agent(), Command::AddMaterial { id, material })?;
        Ok(id)
    }
    /// Adds the ASCE 7 combinations the load cases can form and the model
    /// does not have yet, as one undo step. Cases are matched by load type.
    pub fn generate_combinations(
        &mut self,
        edition: asce7::Edition,
        method: asce7::Method,
    ) -> Result<Value> {
        let commands = asce7::commands(self.model(), edition, method);
        let names: Vec<String> = commands
            .iter()
            .filter_map(|c| match c {
                Command::AddCombination { combination, .. } => Some(combination.name.clone()),
                _ => None,
            })
            .collect();
        if names.is_empty() {
            let typed = self
                .model()
                .load_cases
                .values()
                .any(|c| c.load_type != LoadType::Other);
            let note = if typed {
                "no new combinations: the model already has every one its load cases can form"
            } else {
                "no load case has a load_type the generator uses; give the cases one (dead, live, \
                 wind, ...) with update_load_case, then generate again"
            };
            return Ok(json!({"added": names, "note": note}));
        }
        self.edit(self.this_agent(), Command::Batch { commands })?;
        Ok(json!({"added": names}))
    }
    /// Validates and compiles. Problems come back named, never as indices.
    pub fn compile(&mut self) -> Result<Value> {
        match oa_model::compile(self.model()) {
            Ok(compiled) => {
                let summary = json!({
                    "ok": true,
                    "solver_nodes": compiled.solver.nodes.len(),
                    "solver_frames": compiled.solver.frames.len(),
                    "synthetic_masters": compiled.mapping.synthetic_masters.len(),
                    "content_hash": compiled.content_hash(),
                });
                self.compiled = Some(compiled);
                Ok(summary)
            }
            Err(problems) => Ok(json!({
                "ok": false,
                "problems": problems.iter().map(|p| json!({"entity": p.entity, "name": p.name, "message": p.message})).collect::<Vec<_>>(),
            })),
        }
    }
    fn compiled(&mut self) -> Result<&Compiled> {
        if self.compiled.is_none() {
            let v = self.compile()?;
            if v["ok"] == false {
                return Err(SessionError::Problems(v["problems"].to_string()));
            }
        }
        Ok(self.compiled.as_ref().unwrap())
    }
    /// Runs a static analysis into a result store. `store_path` of None keeps it in memory.
    pub fn analyze(
        &mut self,
        options: StaticOptions,
        store_path: Option<PathBuf>,
    ) -> Result<Value> {
        let compiled = self.compiled()?.clone();
        let mut store = match &store_path {
            Some(p) => ResultStore::create(p, &compiled.solver, &options)?,
            None => ResultStore::create_in_memory(&compiled.solver, &options)?,
        };
        let mut copy = self.keep_in_memory.then(InMemoryResults::default);
        let mut tee = Tee {
            store: &mut store,
            copy: copy.as_mut(),
        };
        oa_core::analyze_static_into(&compiled.solver, &options, &mut tee)?;
        self.fresh = copy;
        oa_model::store::attach(&compiled, &store)?;
        for (name, select) in unit_views() {
            store.define_view(&name, &select)?;
        }
        let combos: Vec<Value> = store
            .combinations()
            .iter()
            .map(|name| {
                let c = store.combination(name).unwrap();
                json!({"name": name, "iterations": c.iterations, "relative_residual": c.relative_residual})
            })
            .collect();
        self.store = Some(store);
        Ok(json!({
            "combinations": combos,
            "store": store_path,
            "cracked_stiffness_factor": options.cracked_stiffness_factor,
        }))
    }
    fn store(&self) -> Result<(&Compiled, &ResultStore)> {
        match (&self.compiled, &self.store) {
            (Some(c), Some(s)) => Ok((c, s)),
            _ => Err(SessionError::Invalid(
                "no current results; run analyze first (edits discard results)".into(),
            )),
        }
    }
    fn index_of(&self, compiled: &Compiled, id: EntityId, quantity: Quantity) -> Result<usize> {
        let table = match quantity {
            Quantity::Displacement | Quantity::Reaction => &compiled.mapping.node_index,
            Quantity::FrameForce => &compiled.mapping.frame_index,
        };
        table.get(&id).copied().ok_or_else(|| {
            SessionError::Invalid(format!(
                "{} is not a {} entity",
                self.model().describe(id),
                match quantity {
                    Quantity::FrameForce => "frame",
                    _ => "node",
                }
            ))
        })
    }
    fn envelope_value(&self, quantity: Quantity, index: usize, component: usize) -> Result<Value> {
        let (_, store) = self.store()?;
        let e = match quantity {
            Quantity::Displacement => store.envelope_displacement(index, component)?,
            Quantity::Reaction => store.envelope_reaction(index, component)?,
            Quantity::FrameForce => store.envelope_frame_force(index, component)?,
        };
        let role = role_of(quantity, component);
        Ok(json!({
            "unit": UNITS.symbol(role),
            "minimum": {"value": display(role, e.minimum.value), "combination": e.minimum.combination},
            "maximum": {"value": display(role, e.maximum.value), "combination": e.maximum.combination},
        }))
    }
    /// Envelope over all combinations for one entity, with the governing combination.
    pub fn envelope(&self, id: EntityId, quantity: Quantity, component: &str) -> Result<Value> {
        let (compiled, _) = self.store()?;
        let index = self.index_of(compiled, id, quantity)?;
        let c = component_index(quantity, component)?;
        let mut v = self.envelope_value(quantity, index, c)?;
        v["entity"] = json!({"id": id, "name": self.model().name_of(id)});
        Ok(v)
    }
    /// Envelopes for every matching member of a group, sorted by largest magnitude.
    pub fn group_envelope(
        &self,
        group: EntityId,
        quantity: Quantity,
        component: &str,
        limit: usize,
    ) -> Result<Value> {
        let (compiled, _) = self.store()?;
        let c = component_index(quantity, component)?;
        let mut rows = vec![];
        for id in self.model().group_members(group) {
            if let Ok(index) = self.index_of(compiled, id, quantity) {
                let v = self.envelope_value(quantity, index, c)?;
                let magnitude = v["minimum"]["value"]
                    .as_f64()
                    .unwrap_or(0.0)
                    .abs()
                    .max(v["maximum"]["value"].as_f64().unwrap_or(0.0).abs());
                rows.push((
                    magnitude,
                    json!({"id": id, "name": self.model().name_of(id), "envelope": v}),
                ));
            }
        }
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        let total = rows.len();
        Ok(json!({
            "total": total,
            "rows": rows.into_iter().take(limit).map(|(_, v)| v).collect::<Vec<_>>(),
        }))
    }
    /// Displacement difference between two nodes, such as storey drift. The
    /// ratio divides by the nodes' actual Z separation, not by a level
    /// height.
    pub fn drift(&self, upper: EntityId, lower: EntityId, component: &str) -> Result<Value> {
        let (compiled, store) = self.store()?;
        let u = self.index_of(compiled, upper, Quantity::Displacement)?;
        let l = self.index_of(compiled, lower, Quantity::Displacement)?;
        let c = component_index(Quantity::Displacement, component)?;
        let e = store.envelope_drift(u, l, c)?;
        let height = (self.model().nodes[&upper].position[2].si()
            - self.model().nodes[&lower].position[2].si())
        .abs();
        // The ratio is dimensionless, so it is formed in SI before the
        // displacement is shown in inches.
        let ratio = |v: f64| if height > 0.0 { Some(v / height) } else { None };
        let role = role_of(Quantity::Displacement, c);
        Ok(json!({
            "upper": self.model().name_of(upper), "lower": self.model().name_of(lower),
            "height": display(Role::Length, height),
            "unit": UNITS.symbol(role),
            "minimum": {"value": display(role, e.minimum.value), "combination": e.minimum.combination, "ratio": ratio(e.minimum.value)},
            "maximum": {"value": display(role, e.maximum.value), "combination": e.maximum.combination, "ratio": ratio(e.maximum.value)},
        }))
    }
    /// Read-only SQL over the result store. Node and frame columns hold solver
    /// indices; `entity_indices` translates.
    pub fn query(&self, sql: &str, limit: usize) -> Result<Value> {
        let (_, store) = self.store()?;
        let table = store.sql(sql, limit.clamp(1, 1000)).map_err(|e| match e {
            // SQLite says "not authorized" for a denied statement and
            // "access to <column> is prohibited" for a denied column read.
            oa_results::Error::Sqlite(ref inner)
                if inner.to_string().contains("not authorized")
                    || inner.to_string().contains("is prohibited") =>
            {
                SessionError::Invalid(
                    "only read-only statements over the plain table names (displacements, \
                     reactions, frame_results, shell_results, combinations, run) are allowed; \
                     those answer in US customary units, and main.<table> is refused because it \
                     would return SI"
                        .into(),
                )
            }
            other => other.into(),
        })?;
        Ok(serde_json::to_value(table)?)
    }
    pub fn entity_indices(&mut self, ids: &[EntityId]) -> Result<Value> {
        let model_names: Vec<Option<String>> = ids
            .iter()
            .map(|id| self.model().name_of(*id).map(str::to_string))
            .collect();
        let compiled = self.compiled()?;
        Ok(ids
            .iter()
            .zip(model_names)
            .map(|(id, name)| {
                json!({
                    "id": id, "name": name,
                    "node": compiled.mapping.node_index.get(id),
                    "frame": compiled.mapping.frame_index.get(id),
                    "shell": compiled.mapping.shell_index.get(id),
                })
            })
            .collect())
    }

    /// Natural modes: periods, frequencies, and mass participation. Mode
    /// shapes are left out; they grow with the model.
    /// Modes with the named mass source, or the default one.
    pub fn modal(&mut self, modes: usize, mass_source: Option<&str>) -> Result<Value> {
        let options = oa_core::ModalOptions {
            modes,
            ..Default::default()
        };
        let solver = self.solver_with(mass_source)?;
        let result = oa_core::analyze_modal(&solver, &options)?;
        Ok(modal_summary(&result))
    }
    /// A response-spectrum run along one direction. The peaks are kept for
    /// [`Session::spectrum_peaks`] until the model changes.
    pub fn response_spectrum(&mut self, request: &SpectrumRequest) -> Result<Value> {
        let g = self.model().gravity.si();
        let direction = request.direction.vector()?;
        let options = oa_core::SpectrumOptions {
            modal: oa_core::ModalOptions {
                modes: request.modes,
                ..Default::default()
            },
            spectrum: request
                .spectrum
                .iter()
                .map(|&[period_seconds, sa]| oa_core::SpectrumPoint {
                    period_seconds,
                    acceleration: Acceleration::from_si(sa * g),
                })
                .collect(),
            direction,
            damping: request.damping,
            combination: match request.combination {
                ModalSum::Cqc => oa_core::ModalCombination::Cqc,
                ModalSum::Srss => oa_core::ModalCombination::Srss,
            },
            minimum_mass_ratio: request.minimum_mass_ratio,
            ..Default::default()
        };
        let solver = self.solver_with(request.mass_source.as_deref())?;
        let result = oa_core::analyze_spectrum(&solver, &options)?;
        let base: Vec<f64> = result
            .base_reaction
            .iter()
            .enumerate()
            .map(|(i, v)| display(if i < 3 { Role::Force } else { Role::Moment }, *v))
            .collect();
        let mut summary = json!({
            "direction": result.direction,
            "combination": request.combination,
            "captured_mass_ratio": result.captured_mass_ratio,
            "base_reaction": {
                "fx": base[0], "fy": base[1], "fz": base[2],
                "mx": base[3], "my": base[4], "mz": base[5],
                "units": [UNITS.symbol(Role::Force), UNITS.symbol(Role::Moment)],
            },
            "modal": modal_summary(&result.modal),
        });
        self.spectrum = Some(result);
        summary["largest_displacements"] = ["ux", "uy", "uz"]
            .iter()
            .map(|c| self.spectrum_peaks(Quantity::Displacement, c, None, None, 1))
            .collect::<Result<Vec<_>>>()?
            .into();
        Ok(summary)
    }
    /// Peaks from the last response-spectrum run, largest first, for one
    /// entity, a group's members, or every node or frame. Peaks from a modal
    /// combination carry no sign, so they are magnitudes.
    pub fn spectrum_peaks(
        &self,
        quantity: Quantity,
        component: &str,
        id: Option<EntityId>,
        group: Option<EntityId>,
        limit: usize,
    ) -> Result<Value> {
        let (Some(compiled), Some(result)) = (&self.compiled, &self.spectrum) else {
            return Err(SessionError::Invalid(
                "no current spectrum results; run response_spectrum first (edits discard results)"
                    .into(),
            ));
        };
        let c = component_index(quantity, component)?;
        let ids: Vec<EntityId> = match (id, group) {
            (Some(id), _) => vec![id],
            (None, Some(group)) => self.model().group_members(group),
            (None, None) => match quantity {
                Quantity::FrameForce => compiled.mapping.frame_index.keys().copied().collect(),
                _ => compiled.mapping.node_index.keys().copied().collect(),
            },
        };
        let missing = || SessionError::Invalid(format!("the run kept no {quantity:?} results"));
        let mut rows = vec![];
        for member in ids {
            let index = match self.index_of(compiled, member, quantity) {
                Ok(index) => index,
                Err(e) if id.is_some() => return Err(e),
                // A group can mix kinds; members of the other kind are skipped.
                Err(_) => continue,
            };
            let value = match quantity {
                Quantity::Displacement => result.displacements.as_ref().ok_or_else(missing)?[index][c],
                Quantity::Reaction => result.reactions.as_ref().ok_or_else(missing)?[index][c],
                Quantity::FrameForce => result.frame_end_forces.as_ref().ok_or_else(missing)?[index][c],
            };
            rows.push((member, value.abs()));
        }
        rows.sort_by(|a, b| b.1.total_cmp(&a.1));
        let role = role_of(quantity, c);
        let total = rows.len();
        Ok(json!({
            "component": component, "unit": UNITS.symbol(role), "total": total,
            "rows": rows.into_iter().take(limit.max(1)).map(|(id, v)| json!({
                "id": id, "name": self.model().name_of(id), "peak": display(role, v),
            })).collect::<Vec<_>>(),
        }))
    }
}

fn modal_summary(r: &oa_core::ModalResult) -> Value {
    json!({
        "units": {"period": "s", "frequency": "Hz", "mass": UNITS.symbol(Role::Mass)},
        "modes": r.modes.iter().enumerate().map(|(i, m)| json!({
            "mode": i + 1,
            "period": m.period_seconds,
            "frequency": m.frequency_hz,
            "mass_ratio": {"x": m.mass_ratio[0], "y": m.mass_ratio[1], "z": m.mass_ratio[2]},
        })).collect::<Vec<_>>(),
        "cumulative_mass_ratio": {
            "x": r.cumulative_mass_ratio[0], "y": r.cumulative_mass_ratio[1], "z": r.cumulative_mass_ratio[2],
        },
        "total_free_mass": r.total_free_mass.map(|m| display(Role::Mass, m)),
        "maximum_mass_orthogonality_error": r.maximum_mass_orthogonality_error,
        // The solver fails the run when the count disagrees, so a count here
        // certifies that no lower mode was skipped.
        "sturm_check": r.sturm_count_below_cutoff.map(|n| json!({
            "passed": true, "modes_below_cutoff": n, "cutoff_hz": r.sturm_cutoff_hz,
        })),
    })
}

/// How a response-spectrum run sums its modes.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModalSum {
    /// Complete quadratic combination, which accounts for closely spaced modes.
    #[default]
    Cqc,
    /// Square root of the sum of the squares.
    Srss,
}

/// A global direction of excitation.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum Direction {
    /// "x", "y", or "z".
    Axis(String),
    /// A global vector such as [1, 1, 0]; the solver normalizes it.
    Vector([f64; 3]),
}
impl Direction {
    fn vector(&self) -> Result<[f64; 3]> {
        match self {
            Self::Vector(v) => Ok(*v),
            Self::Axis(a) => match a.to_ascii_lowercase().as_str() {
                "x" => Ok([1.0, 0.0, 0.0]),
                "y" => Ok([0.0, 1.0, 0.0]),
                "z" => Ok([0.0, 0.0, 1.0]),
                _ => Err(SessionError::Invalid(format!(
                    "direction {a:?}; use \"x\", \"y\", \"z\", or a vector"
                ))),
            },
        }
    }
}

fn default_damping() -> f64 {
    0.05
}
fn default_spectrum_modes() -> usize {
    12
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SpectrumRequest {
    /// The design spectrum as [period in s, spectral acceleration as a
    /// fraction of g] pairs with periods strictly increasing. It must span
    /// every computed mode's period, so start it at 0 s.
    pub spectrum: Vec<[f64; 2]>,
    /// "x", "y", "z", or a global vector such as [1, 1, 0].
    pub direction: Direction,
    /// The damping ratio the spectrum is for, which CQC also uses. Default 0.05.
    #[serde(default = "default_damping")]
    pub damping: f64,
    /// "cqc" (default) or "srss".
    #[serde(default)]
    pub combination: ModalSum,
    /// Modes to compute and combine. Default 12.
    #[serde(default = "default_spectrum_modes")]
    pub modes: usize,
    /// Refuse the run when the mass captured along the direction is below
    /// this ratio, such as 0.9.
    pub minimum_mass_ratio: Option<f64>,
    /// The mass source to use, by name; the model's default when absent.
    #[serde(default)]
    pub mass_source: Option<String>,
}

/// Reference text for the `apply_commands` tool: one entry per command with a
/// minimal JSON example. Kept as data so the agent can read it once.
pub const COMMAND_REFERENCE: &str = r#"Each command is a JSON object with a "command" field. Ids come from next_ids.
Units are US customary: coordinates, elevations and load positions in ft, shell thickness in in, section area in in² and
moments of area in in⁴, E and material strengths in ksi, density in pcf, forces in kip, moments in kip·ft, line loads in kip/ft,
surface pressure in psf, springs in kip/ft and kip·ft/rad, nodal mass in kip·s²/ft, prescribed displacements
in in and rad, roll in degrees, gravity in ft/s².
Z is up. X and Y are the plan axes. A level is a plane of constant Z, and every node binds to one level: its
offset above the level is position z minus the level elevation. A new model has one level, "Base" at 0.
list_entities with kind "level" gives each level's id, elevation, and storey height below, lowest first.

add_level     {"command":"add_level","id":11,"level":{"name":"Level 2","elevation":12}}
update_level  {"command":"update_level","id":11,"level":{"name":"L2","elevation":12}}
              renames, or re-datums the level in place: its nodes keep their coordinates and their offsets change
set_level_elevation {"command":"set_level_elevation","id":11,"elevation":14,"scope":"this_and_above"}
              moves the datum with its bound nodes, keeping every offset. scope "this_level" holds every other level
              still; "this_and_above" carries the higher levels and their nodes too, so the storey heights above are
              kept. For a storey height, set elevation = elevation of the level below + height, with this_and_above.
              refused if the move would cross or land on a level that is not moving, or make geometry invalid that
              was valid before (a load station past a shortened member, for example). Undo restores exact values.
remove_level  {"command":"remove_level","id":11}    refused while a node binds to it (update_node to another level
              first, which keeps the node's coordinates) and for the last level
add_node      {"command":"add_node","id":1,"node":{"name":"N1","level":11,"position":[0,0,12],"restrained":[true,true,true,true,true,true]}}
              level is required. optional node fields: prescribed, mass [kip·s²/ft x3], mass_inertia,
              spring_translation [kip/ft x3], spring_rotation
update_node   {"command":"update_node","id":1,"node":{...full node...}}
remove_node   {"command":"remove_node","id":1}     (refused while a frame, shell, diaphragm or load references it)
add_material  {"command":"add_material","id":2,"material":{"name":"steel","young":29000,"poisson":0.3,"density":490,"fy":50,"fu":65}}
              optional strengths, kept for design and unused by the solver: fy and fu for steel (fu at least fy),
              fc (f'c) for concrete. add_material_from_library fills them for the bundled grades
add_section   {"command":"add_section","id":3,"section":{"name":"col","area":26.5,"iy":362,"iz":999,"torsion":4.06}}
              optional shear_y and shear_z [in²], the shear areas along local y (bending with iz) and z: a plane with
              one deforms in shear too (Timoshenko), one without is rigid in shear. add_section_from_library fills
              them: d·tw along the web, 5/3·bf·tf across two flanges, 2·t·h for tube walls, (0.5+0.8·t/OD)·A if round
add_frame     {"command":"add_frame","id":4,"frame":{"name":"C1","nodes":[1,5],"material":2,"section":3}}
              optional: releases [12 bools], behavior "tension_only"|"compression_only", roll, local_y,
              modifiers {"area","shear_y","shear_z","torsion","iy","iz"}: stiffness multipliers on those section
              properties, each 1 unless given; mass and self-weight are unchanged. ACI 318 cracked sections:
              beams {"iy":0.35,"iz":0.35}, columns {"iy":0.7,"iz":0.7}. modifiers "mass" and "weight" (each 1
              unless given, 0 allowed) scale the member's own mass and its self-weight; 0 on both leaves out a
              member another one already carries, such as a beam under a slab modelled with the slab's weight
              optional offsets {"end":[i,j],"rigid_zone":0.5,"joint":[[x,y,z],[x,y,z]],"axes":"global"|"local"}, in in:
              end is the length at each end inside the joint (half the depth of the column a beam frames into);
              section forces and diagrams cover the clear length between them, and rigid_zone (0 to 1, default 0)
              of each is rigid in bending and shear, as ETABS's rigid-zone factor; axial and torsional stiffness
              stay the whole length's. Releases act at the ends of the flexible part. The transverse part of a
              member load inside a rigid zone goes straight to the node. joint moves each end of the member off
              its node through a rigid link, in global axes or the member's local axes; member load positions
              then run along the moved member. optional cardinal_point: which point of the section sits on the
              line between the moved ends, in that line's local axes, as ETABS numbers 1-10:
              "bottom_left","bottom_center","bottom_right","middle_left",
              "middle_center","middle_right","top_left","top_center","top_right","centroid" (default). Seen
              from end I with local y up: top is +y, right is +z. A beam with local_y [0,0,1] whose nodes sit at
              the slab top takes "top_center" and hangs below them. It needs a section from the shape library,
              whose depth and width it measures; compile reports it on a section without a shape
split_frames  {"command":"split_frames","frames":[4],"nodes":[5]}
              a frame only connects to the nodes at its two ends: a node placed on a span, or a beam drawn to land
              on a girder's midspan, is not joined until the frame is split there. This splits the listed frames at
              the listed nodes lying on their spans (within 0.1 mm, about 0.004 in, of the axis); either list empty or left out means
              all of them, so {"command":"split_frames"} splits every frame at every node on it. The frame keeps
              its id and name as the first piece; the others are new frames named "C1-2", "C1-3", ... with its
              material, section, orientation and modifiers, its groups, and its share of every member load (point
              loads by position, distributed loads cut and interpolated). End releases and end offsets stay at the
              outer ends; joint offsets are interpolated to the new joints so the pieces stay on the moved line.
add_shell     {"command":"add_shell","id":6,"shell":{"name":"S1","nodes":[1,2,3,4],"material":2,"thickness":8}}
              optional local_x [x,y,z]: reference for local x, projected into the shell's plane; by default local x
              runs from the first node to the second. Modifiers act, and stresses are reported, in these axes.
              optional modifiers {"membrane_x","membrane_y","membrane_shear","bending"}: stiffness multipliers on
              in-plane normal stiffness along local x (f11) and y (f22), in-plane shear (f12) and plate bending
              (m11, m22, m12), each 1 unless given. ACI 318: walls {"membrane_x":0.7,"membrane_y":0.7} uncracked
              or 0.35 cracked, flat slabs {"bending":0.25}. "mass" and "weight" as for frames
add_diaphragm {"command":"add_diaphragm","id":7,"diaphragm":{"name":"D1","nodes":[5,6,7],"normal":"z"}}   master optional
add_load_case {"command":"add_load_case","id":8,"load_case":{"name":"wind","load_type":"wind","nodal":[{"node":5,"force":[10,0,0]}],
               "member":[{"type":"distributed","member":4,"start":0,"end":20,"start_load":[0,0,-1],"end_load":[0,0,-1],"axes":"global"}],
               "surface":[{"shell":6,"pressure":-50}],"self_weight":[0,0,-1]}}
              load_type is what generate_combinations matches on: dead, live, roof_live, snow, rain, wind, earthquake,
              earth_pressure, fluid, self_straining, flood, ice, wind_on_ice, or other (the default, never generated).
              self_weight is a multiple of g per axis. surface pressure acts along the shell normal, which follows
              its nodes by the right-hand rule. Loads are not mass unless a mass source lists the case.
add_mass_source {"command":"add_mass_source","id":14,"mass_source":{"name":"seismic","cases":[[8,1.0],[13,0.25]]}}
              what modal and response_spectrum take as mass. Node mass always counts; element_mass (default true)
              adds the members' own mass from density; each [case id, multiplier] adds that case's downward (-Z)
              load divided by g, in X, Y and Z. ASCE 7 12.7.2: superimposed dead and partitions at 1.0, storage
              live at 0.25. A listed case with self-weight while element_mass is true would count member mass
              twice, and compile reports it against the source; set element_mass false to take member mass from
              a dead case's self-weight instead. optional: lateral (default true) keeps X and Y mass and rotation
              about Z, vertical (default true) keeps Z mass and rotation about X and Y; "vertical":false keeps
              modal runs from spending modes on beams bouncing. lump_to_levels (default false) moves all the
              lateral mass of each node that is not on a level onto the node directly below or above it on the
              NEAREST level, as ETABS does: mass at z=1 between levels at 0 and 4 goes wholly to level 0. Only a
              node exactly halfway between two levels splits 50/50. Compile reports a node with no node directly
              below or above it on that level. A case a source lists cannot be removed.
set_default_mass_source {"command":"set_default_mass_source","id":14}   the source modal and response_spectrum use
              unless given mass_source by name; id null goes back to node and element mass only. The default
              source cannot be removed. describe_model lists every source and names the default.
add_combination {"command":"add_combination","id":9,"combination":{"name":"1.2D+1.6W","terms":[[8,1.6]]}}
add_group     {"command":"add_group","id":10,"group":{"name":"roof","members":[5,6]}}
add_underlay  {"command":"add_underlay","id":12,"underlay":{"name":"grid","level":11,"origin":[0,0],"segments":[[[0,0],[20,0]],[[0,0],[0,20]]]}}
              plan line work on a level, drawn by the GUI as a tracing reference; the solver never sees it
add_grid_line {"command":"add_grid_line","id":15,"grid_line":{"name":"A","start":[0,-5],"end":[0,55]}}
              a plan grid line: name is its bubble label, start the bubble end, both ends in plan [x, y] and apart.
              It stands for a vertical plane through every level; the GUI draws it on the level in view and the draw
              tools snap to it and its crossings. The solver never sees it. A rectangular grid is one batch of these:
              X grid lines A, B, C... at constant x running along Y, Y grid lines 1, 2, 3... at constant y running
              along X, each a little past the outermost lines it crosses. Labels are names, so each is used once
update_*, remove_* exist for every kind. set_gravity {"command":"set_gravity","gravity":32.174}
set_metadata  {"command":"set_metadata","metadata":{"name":"Office block"}}   replaces the whole metadata record
batch         {"command":"batch","commands":[...]}   all or nothing (apply_commands already wraps its list in a batch)
"#;
