//! Transport-independent session behind the MCP tools. Every tool is a
//! method here that takes plain arguments and returns JSON, so the whole
//! surface is testable without an agent or a transport.
use oa_core::StaticOptions;
use oa_model::{compile::Compiled, *};
use oa_results::ResultStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

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
pub struct Session {
    pub editor: Editor,
    pub path: Option<PathBuf>,
    compiled: Option<Compiled>,
    store: Option<ResultStore>,
}
impl Default for Session {
    fn default() -> Self {
        Self::new(Model::default())
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
        }
    }
    fn invalidate(&mut self) {
        self.compiled = None;
        self.store = None;
    }
    fn model(&self) -> &Model {
        &self.editor.model
    }

    /// Sizes and names an agent needs before it can do anything else.
    pub fn describe(&self) -> Value {
        let m = self.model();
        let names = |it: Vec<&str>| Value::from(it);
        json!({
            "name": m.metadata.name,
            "path": self.path,
            "gravity": m.gravity.si(),
            "counts": {
                "nodes": m.nodes.len(), "materials": m.materials.len(), "sections": m.sections.len(),
                "frames": m.frames.len(), "shells": m.shells.len(), "diaphragms": m.diaphragms.len(),
                "load_cases": m.load_cases.len(), "combinations": m.combinations.len(), "groups": m.groups.len(),
            },
            "load_cases": names(m.load_cases.values().map(|c| c.name.as_str()).collect()),
            "combinations": names(m.combinations.values().map(|c| c.name.as_str()).collect()),
            "groups": m.groups.iter().map(|(id, g)| json!({"id": id, "name": g.name, "size": g.members.len()})).collect::<Vec<_>>(),
            "compiled": self.compiled.is_some(),
            "results": self.store.as_ref().map(|s| json!({"combinations": s.combinations(), "current": true})),
            "can_undo": self.editor.can_undo(),
            "can_redo": self.editor.can_redo(),
        })
    }
    /// Compact rows for one entity kind, optionally filtered by a name substring.
    pub fn list(&self, kind: EntityKind, filter: Option<&str>, limit: usize) -> Value {
        let m = self.model();
        let matches = |name: &str| filter.is_none_or(|f| name.contains(f));
        let mut rows = vec![];
        let mut total = 0;
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
            EntityKind::Node => rows!(
                m.nodes,
                |id, n| json!({"id": id, "name": n.name, "position": n.position, "restrained": n.restrained})
            ),
            EntityKind::Material => rows!(
                m.materials,
                |id, e| json!({"id": id, "name": e.name, "young": e.young.si()})
            ),
            EntityKind::Section => rows!(
                m.sections,
                |id, e| json!({"id": id, "name": e.name, "area": e.area.si(), "provenance": e.provenance.as_ref().map(|p| &p.designation)})
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
        }
        json!({"total": total, "rows": rows, "truncated": total > rows.len()})
    }
    pub fn get(&self, id: EntityId) -> Result<Value> {
        let m = self.model();
        let kind = m.kind_of(id).ok_or(ModelError::NotFound(id))?;
        let value = match kind {
            EntityKind::Node => serde_json::to_value(&m.nodes[&id])?,
            EntityKind::Material => serde_json::to_value(&m.materials[&id])?,
            EntityKind::Section => serde_json::to_value(&m.sections[&id])?,
            EntityKind::Frame => serde_json::to_value(&m.frames[&id])?,
            EntityKind::Shell => serde_json::to_value(&m.shells[&id])?,
            EntityKind::Diaphragm => serde_json::to_value(&m.diaphragms[&id])?,
            EntityKind::LoadCase => serde_json::to_value(&m.load_cases[&id])?,
            EntityKind::Combination => serde_json::to_value(&m.combinations[&id])?,
            EntityKind::Group => serde_json::to_value(&m.groups[&id])?,
        };
        Ok(json!({"id": id, "kind": kind, "entity": value}))
    }
    pub fn find(&self, kind: EntityKind, name: &str) -> Option<EntityId> {
        let m = self.model();
        match kind {
            EntityKind::Node => m.find::<Node>(name),
            EntityKind::Material => m.find::<Material>(name),
            EntityKind::Section => m.find::<Section>(name),
            EntityKind::Frame => m.find::<Frame>(name),
            EntityKind::Shell => m.find::<Shell>(name),
            EntityKind::Diaphragm => m.find::<Diaphragm>(name),
            EntityKind::LoadCase => m.find::<LoadCase>(name),
            EntityKind::Combination => m.find::<Combination>(name),
            EntityKind::Group => m.find::<Group>(name),
        }
    }
    /// Fresh ids for commands that add entities.
    pub fn next_ids(&mut self, count: usize) -> Vec<EntityId> {
        (0..count.clamp(1, 1000))
            .map(|_| self.editor.model.allocate())
            .collect()
    }
    /// Applies commands atomically. Any change invalidates compilation and results.
    pub fn apply(&mut self, commands: Vec<Command>) -> Result<Value> {
        let count = commands.len();
        self.editor.apply(Command::Batch { commands })?;
        self.invalidate();
        Ok(json!({"applied": count, "counts": self.describe()["counts"]}))
    }
    pub fn undo(&mut self) -> Result<bool> {
        let done = self.editor.undo()?;
        if done {
            self.invalidate();
        }
        Ok(done)
    }
    pub fn redo(&mut self) -> Result<bool> {
        let done = self.editor.redo()?;
        if done {
            self.invalidate();
        }
        Ok(done)
    }
    pub fn new_model(&mut self, name: &str) {
        let mut model = Model::default();
        model.metadata.name = name.into();
        *self = Self::new(model);
    }
    pub fn load(&mut self, path: PathBuf) -> Result<Value> {
        let model = from_json(&std::fs::read_to_string(&path)?)?;
        *self = Self::new(model);
        self.path = Some(path);
        Ok(self.describe())
    }
    pub fn save(&mut self, path: Option<PathBuf>) -> Result<PathBuf> {
        let path = path
            .or_else(|| self.path.clone())
            .ok_or_else(|| SessionError::Invalid("no path given and the model has none".into()))?;
        oa_model::save_json(self.model(), &path)?;
        self.path = Some(path.clone());
        Ok(path)
    }
    pub fn library(&self) -> Value {
        let lib = Library::starter();
        json!({
            "library": lib.name, "version": lib.version, "note": lib.note,
            "sections": lib.section_designations(),
            "materials": lib.materials.iter().map(|m| m.designation.as_str()).collect::<Vec<_>>(),
        })
    }
    pub fn add_section_from_library(&mut self, designation: &str, name: &str) -> Result<EntityId> {
        let section = Library::starter()
            .section(designation, name)
            .ok_or_else(|| {
                SessionError::Invalid(format!("no section {designation:?} in the library"))
            })?;
        let id = self.editor.model.allocate();
        self.editor.apply(Command::AddSection { id, section })?;
        self.invalidate();
        Ok(id)
    }
    pub fn add_material_from_library(&mut self, designation: &str, name: &str) -> Result<EntityId> {
        let material = Library::starter()
            .material(designation, name)
            .ok_or_else(|| {
                SessionError::Invalid(format!("no material {designation:?} in the library"))
            })?;
        let id = self.editor.model.allocate();
        self.editor.apply(Command::AddMaterial { id, material })?;
        self.invalidate();
        Ok(id)
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
        oa_core::analyze_static_into(&compiled.solver, &options, &mut store)?;
        oa_model::store::attach(&compiled, &store)?;
        let combos: Vec<Value> = store
            .combinations()
            .iter()
            .map(|name| {
                let c = store.combination(name).unwrap();
                json!({"name": name, "iterations": c.iterations, "relative_residual": c.relative_residual})
            })
            .collect();
        self.store = Some(store);
        Ok(json!({"combinations": combos, "store": store_path}))
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
        Ok(json!({
            "minimum": {"value": e.minimum.value, "combination": e.minimum.combination},
            "maximum": {"value": e.maximum.value, "combination": e.maximum.combination},
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
    /// Displacement difference between two nodes, such as storey drift.
    pub fn drift(&self, upper: EntityId, lower: EntityId, component: &str) -> Result<Value> {
        let (compiled, store) = self.store()?;
        let u = self.index_of(compiled, upper, Quantity::Displacement)?;
        let l = self.index_of(compiled, lower, Quantity::Displacement)?;
        let c = component_index(Quantity::Displacement, component)?;
        let e = store.envelope_drift(u, l, c)?;
        let height = (self.model().nodes[&upper].position[1].si()
            - self.model().nodes[&lower].position[1].si())
        .abs();
        let ratio = |v: f64| if height > 0.0 { Some(v / height) } else { None };
        Ok(json!({
            "upper": self.model().name_of(upper), "lower": self.model().name_of(lower),
            "minimum": {"value": e.minimum.value, "combination": e.minimum.combination, "ratio": ratio(e.minimum.value)},
            "maximum": {"value": e.maximum.value, "combination": e.maximum.combination, "ratio": ratio(e.maximum.value)},
        }))
    }
    /// Read-only SQL over the result store. Node and frame columns hold solver
    /// indices; `entity_indices` translates.
    pub fn query(&self, sql: &str, limit: usize) -> Result<Value> {
        let (_, store) = self.store()?;
        Ok(serde_json::to_value(store.sql(sql, limit.clamp(1, 1000))?)?)
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
}

/// Reference text for the `apply_commands` tool: one entry per command with a
/// minimal JSON example. Kept as data so the agent can read it once.
pub const COMMAND_REFERENCE: &str = r#"Each command is a JSON object with a "command" field. Ids come from next_ids.
Units are SI: metres, newtons, kilograms, pascals, radians. Y is up in the examples.

add_node      {"command":"add_node","id":1,"node":{"name":"N1","position":[0,0,0],"restrained":[true,true,true,true,true,true]}}
              optional node fields: prescribed, mass [kg x3], mass_inertia, spring_translation [N/m x3], spring_rotation
update_node   {"command":"update_node","id":1,"node":{...full node...}}
remove_node   {"command":"remove_node","id":1}     (refused while a frame, shell, diaphragm or load references it)
add_material  {"command":"add_material","id":2,"material":{"name":"steel","young":2e11,"poisson":0.3,"density":7850}}
add_section   {"command":"add_section","id":3,"section":{"name":"col","area":0.01,"iy":2e-5,"iz":4e-5,"torsion":1e-5}}
add_frame     {"command":"add_frame","id":4,"frame":{"name":"C1","nodes":[1,5],"material":2,"section":3}}
              optional: releases [12 bools], behavior "tension_only"|"compression_only", roll, local_y
add_shell     {"command":"add_shell","id":6,"shell":{"name":"S1","nodes":[1,2,3,4],"material":2,"thickness":0.2}}
add_diaphragm {"command":"add_diaphragm","id":7,"diaphragm":{"name":"L1","nodes":[5,6,7],"normal":"y"}}   master optional
add_load_case {"command":"add_load_case","id":8,"load_case":{"name":"wind","nodal":[{"node":5,"force":[10000,0,0]}],
               "member":[{"type":"distributed","member":4,"start":0,"end":6,"start_load":[0,-10000,0],"end_load":[0,-10000,0],"axes":"global"}],
               "self_weight":[0,-1,0]}}
add_combination {"command":"add_combination","id":9,"combination":{"name":"1.2D+1.6W","terms":[[8,1.6]]}}
add_group     {"command":"add_group","id":10,"group":{"name":"roof","members":[5,6]}}
update_*, remove_* exist for every kind. set_gravity {"command":"set_gravity","gravity":9.80665}
batch         {"command":"batch","commands":[...]}   all or nothing (apply_commands already wraps its list in a batch)
"#;
