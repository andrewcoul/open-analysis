//! The open model: the editor with its history, the file it came from, the
//! current selection, validation problems, and the last analysis. Every view
//! observes this entity and re-renders when it notifies.
use gpui_kit::Context;
use oa_core::units::*;
use oa_model::{
    Combination, Command, Compiled, Editor, EntityId, EntityKind, Frame, LoadCase, Material,
    MemberLoad, Model, ModelError, Node, Problem, Section, compile,
};
use std::path::{Path, PathBuf};

/// Results of the last static analysis, kept only while the model is unchanged.
pub struct Analysis {
    pub compiled: Compiled,
    pub results: oa_core::InMemoryResults,
    /// Index into `results.combinations` shown by the viewport.
    pub combination: usize,
}

pub struct Document {
    editor: Editor,
    path: Option<PathBuf>,
    dirty: bool,
    revision: u64,
    /// In click order, so "frame between the two selected nodes" is well defined.
    selection: Vec<EntityId>,
    problems: Vec<Problem>,
    analysis: Option<Analysis>,
}

impl Document {
    pub fn with_model(model: Model, path: Option<PathBuf>) -> Self {
        let problems = validate(&model);
        Self {
            editor: Editor::new(model),
            path,
            dirty: false,
            revision: 0,
            selection: vec![],
            problems,
            analysis: None,
        }
    }

    pub fn model(&self) -> &Model {
        &self.editor.model
    }
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
    /// Bumps on every model change. Views use it to invalidate derived state.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }
    pub fn analysis(&self) -> Option<&Analysis> {
        self.analysis.as_ref()
    }
    pub fn can_undo(&self) -> bool {
        self.editor.can_undo()
    }
    pub fn can_redo(&self) -> bool {
        self.editor.can_redo()
    }
    pub fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        if self.dirty {
            format!("{name} *")
        } else {
            name
        }
    }

    // MARK: Selection

    pub fn selection(&self) -> &[EntityId] {
        &self.selection
    }
    pub fn is_selected(&self, id: EntityId) -> bool {
        self.selection.contains(&id)
    }
    /// Selected entities of one kind, in selection order.
    pub fn selected_of(&self, kind: EntityKind) -> Vec<EntityId> {
        self.selection
            .iter()
            .copied()
            .filter(|id| self.model().kind_of(*id) == Some(kind))
            .collect()
    }
    pub fn set_selection(&mut self, ids: Vec<EntityId>, cx: &mut Context<Self>) {
        let model = self.model();
        self.selection = ids
            .into_iter()
            .filter(|id| model.kind_of(*id).is_some())
            .collect();
        cx.notify();
    }
    pub fn toggle_selected(&mut self, id: EntityId, cx: &mut Context<Self>) {
        if let Some(ix) = self.selection.iter().position(|s| *s == id) {
            self.selection.remove(ix);
        } else if self.model().kind_of(id).is_some() {
            self.selection.push(id);
        }
        cx.notify();
    }
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            self.selection.clear();
            cx.notify();
        }
    }

    // MARK: Editing

    pub fn apply(&mut self, command: Command, cx: &mut Context<Self>) -> Result<(), ModelError> {
        self.editor.apply(command)?;
        self.after_edit(cx);
        Ok(())
    }
    pub fn undo(&mut self, cx: &mut Context<Self>) -> Result<bool, ModelError> {
        let done = self.editor.undo()?;
        if done {
            self.after_edit(cx);
        }
        Ok(done)
    }
    pub fn redo(&mut self, cx: &mut Context<Self>) -> Result<bool, ModelError> {
        let done = self.editor.redo()?;
        if done {
            self.after_edit(cx);
        }
        Ok(done)
    }
    fn after_edit(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.revision += 1;
        self.analysis = None;
        self.problems = validate(self.model());
        let model = &self.editor.model;
        self.selection.retain(|id| model.kind_of(*id).is_some());
        cx.notify();
    }

    /// Replaces the whole model, for example after opening a file.
    pub fn replace(&mut self, model: Model, path: Option<PathBuf>, cx: &mut Context<Self>) {
        *self = Self::with_model(model, path);
        self.revision = 1;
        cx.notify();
    }

    pub fn mark_saved(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.path = Some(path);
        self.dirty = false;
        cx.notify();
    }

    // MARK: Analysis

    /// Runs a linear static analysis for every combination. Returns the number
    /// of combinations solved.
    pub fn run_static(&mut self, cx: &mut Context<Self>) -> Result<usize, String> {
        let compiled = compile(self.model()).map_err(|problems| {
            problems
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })?;
        let results = oa_core::analyze_static(&compiled.solver, &oa_core::StaticOptions::default())
            .map_err(|e| e.to_string())?;
        let count = results.combinations.len();
        self.analysis = Some(Analysis {
            compiled,
            results,
            combination: 0,
        });
        cx.notify();
        Ok(count)
    }
    pub fn set_combination(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(a) = &mut self.analysis
            && ix < a.results.combinations.len()
        {
            a.combination = ix;
            cx.notify();
        }
    }
}

fn validate(model: &Model) -> Vec<Problem> {
    compile(model).err().unwrap_or_default()
}

/// A name of the form `prefix + number` that no entity of this kind uses yet.
pub fn unused_name<T: oa_model::model::Entity>(model: &Model, prefix: &str) -> String {
    let count = T::table(model).len();
    (count + 1..)
        .map(|n| format!("{prefix}{n}"))
        .find(|name| model.find::<T>(name).is_none())
        .expect("unbounded")
}

pub fn read_model(path: &Path) -> Result<Model, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    oa_model::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn write_model(model: &Model, path: &Path) -> Result<(), String> {
    oa_model::save_json(model, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A two-storey, two-bay steel moment frame with dead and wind cases, so a
/// new user has something to look at. Z is up.
pub fn example_frame() -> Model {
    let mut m = Model::default();
    m.metadata.name = "Example frame".into();
    let steel = m.insert(Material {
        name: "S355".into(),
        young: Pressure::from_gpa(210.0),
        poisson: 0.3,
        density: MassDensity::from_si(7850.0),
        provenance: None,
    });
    let column = m.insert(Section {
        name: "HEB300".into(),
        area: Area::from_si(1.49e-2),
        iy: SecondMoment::from_si(8.56e-5),
        iz: SecondMoment::from_si(2.517e-4),
        torsion: SecondMoment::from_si(1.85e-6),
        provenance: None,
    });
    let beam = m.insert(Section {
        name: "IPE400".into(),
        area: Area::from_si(8.45e-3),
        iy: SecondMoment::from_si(1.318e-5),
        iz: SecondMoment::from_si(2.313e-4),
        torsion: SecondMoment::from_si(5.1e-7),
        provenance: None,
    });
    let bays_x = 2;
    let bays_y = 1;
    let storeys = 2;
    let (bay, storey) = (6.0, 3.5);
    let mut nodes = vec![];
    for k in 0..=storeys {
        for j in 0..=bays_y {
            for i in 0..=bays_x {
                let position = [
                    Length::from_metres(i as f64 * bay),
                    Length::from_metres(j as f64 * bay),
                    Length::from_metres(k as f64 * storey),
                ];
                let name = format!("N{}", nodes.len() + 1);
                let node = if k == 0 {
                    Node::fixed(name, position)
                } else {
                    Node::new(name, position)
                };
                nodes.push(m.insert(node));
            }
        }
    }
    let index = |i: usize, j: usize, k: usize| {
        nodes[k * (bays_y + 1) * (bays_x + 1) + j * (bays_x + 1) + i]
    };
    let mut frame_count = 0;
    let mut beams = vec![];
    let mut add_frame = |m: &mut Model, a: EntityId, b: EntityId, section: EntityId| {
        frame_count += 1;
        m.insert(Frame::new(
            format!("F{frame_count}"),
            [a, b],
            steel,
            section,
        ))
    };
    for k in 0..storeys {
        for j in 0..=bays_y {
            for i in 0..=bays_x {
                add_frame(&mut m, index(i, j, k), index(i, j, k + 1), column);
            }
        }
    }
    for k in 1..=storeys {
        for j in 0..=bays_y {
            for i in 0..bays_x {
                beams.push(add_frame(&mut m, index(i, j, k), index(i + 1, j, k), beam));
            }
        }
        for j in 0..bays_y {
            for i in 0..=bays_x {
                beams.push(add_frame(&mut m, index(i, j, k), index(i, j + 1, k), beam));
            }
        }
    }
    let mut dead = LoadCase::new("Dead");
    dead.self_weight = [0.0, 0.0, -1.0];
    for b in &beams {
        dead.member.push(MemberLoad::Distributed {
            member: *b,
            start: Length::ZERO,
            end: Length::from_metres(bay),
            start_load: [LineLoad::ZERO, LineLoad::ZERO, LineLoad::from_si(-12_000.0)],
            end_load: [LineLoad::ZERO, LineLoad::ZERO, LineLoad::from_si(-12_000.0)],
            axes: oa_model::Axes::Global,
        });
    }
    let mut wind = LoadCase::new("Wind");
    for k in 1..=storeys {
        for j in 0..=bays_y {
            wind.nodal.push(oa_model::NodalLoad {
                node: index(0, j, k),
                force: [Force::from_kn(15.0 * k as f64), Force::ZERO, Force::ZERO],
                moment: [Moment::ZERO; 3],
            });
        }
    }
    let dead = m.insert(dead);
    let wind = m.insert(wind);
    m.insert(Combination {
        name: "1.35D".into(),
        terms: vec![(dead, 1.35)],
    });
    m.insert(Combination {
        name: "1.0D + 1.5W".into(),
        terms: vec![(dead, 1.0), (wind, 1.5)],
    });
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_frame_compiles() {
        let model = example_frame();
        let compiled = compile(&model).expect("example compiles");
        assert_eq!(compiled.solver.nodes.len(), 18);
        assert_eq!(compiled.solver.combinations.len(), 2);
        let results = oa_core::analyze_static(&compiled.solver, &Default::default()).unwrap();
        assert_eq!(results.combinations.len(), 2);
    }

    #[test]
    fn unused_name_skips_taken_names() {
        let mut model = Model::default();
        model.insert(Node::new("N1", [Length::ZERO; 3]));
        model.insert(Node::new("N3", [Length::ZERO; 3]));
        assert_eq!(unused_name::<Node>(&model, "N"), "N4");
    }
}
