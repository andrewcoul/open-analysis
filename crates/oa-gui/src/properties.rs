//! The property panel: edits the selected entity through inverse-returning
//! commands. Text fields commit on Enter or blur, checkboxes and choices
//! commit at once. With several entities selected it offers bulk assignment.
use crate::actions::{AddDistributedLoad, AddNodalLoad, DeleteSelected};
use crate::document::Document;
use crate::explorer::rows_of;
use crate::text::{UNITS, fmt_num, fmt_q, label, parse_num, parse_q};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use oa_core::units::Length;
use oa_core::units::*;
use oa_model::{
    AxialBehavior, Axis, Command, EntityId, EntityKind, MemberLoad, Model, Role, ShellFormulation,
};
use std::collections::HashMap;

type Choice = Entity<SelectState<SearchableVec<SharedString>>>;

/// What one field shows, independent of the widget that edits it.
enum FieldSpec {
    Text {
        key: String,
        label: SharedString,
        value: String,
    },
    Check {
        key: String,
        group: SharedString,
        label: SharedString,
        value: bool,
    },
    Choice {
        key: String,
        label: SharedString,
        options: Vec<SharedString>,
        selected: Option<usize>,
    },
}
fn text(key: &str, label: &str, value: impl Into<String>) -> FieldSpec {
    FieldSpec::Text {
        key: key.into(),
        label: label.into(),
        value: value.into(),
    }
}
fn num(key: &str, label: &str, value: f64) -> FieldSpec {
    text(key, label, fmt_num(value))
}
/// A quantity field: labelled with its unit, showing the SI value converted.
fn qty(key: &str, name: &str, role: Role, si: f64) -> FieldSpec {
    text(key, &label(name, role), fmt_q(role, si))
}
fn check(key: String, group: &str, label: &str, value: bool) -> FieldSpec {
    FieldSpec::Check {
        key,
        group: group.into(),
        label: label.into(),
        value,
    }
}
fn choice(
    key: &str,
    label: &str,
    options: Vec<SharedString>,
    selected: Option<usize>,
) -> FieldSpec {
    FieldSpec::Choice {
        key: key.into(),
        label: label.into(),
        options,
        selected,
    }
}
fn entity_choice(
    key: &str,
    label: &str,
    model: &Model,
    kind: EntityKind,
    current: Option<EntityId>,
) -> FieldSpec {
    let rows = rows_of(model, kind);
    let selected = current.and_then(|c| rows.iter().position(|(id, _)| *id == c));
    choice(
        key,
        label,
        rows.into_iter().map(|(_, name)| name.into()).collect(),
        selected,
    )
}

const DOF: [&str; 6] = ["Ux", "Uy", "Uz", "Rx", "Ry", "Rz"];
const BEHAVIORS: [(&str, AxialBehavior); 3] = [
    ("Tension and compression", AxialBehavior::Both),
    ("Tension only", AxialBehavior::TensionOnly),
    ("Compression only", AxialBehavior::CompressionOnly),
];
const FORMULATIONS: [(&str, ShellFormulation); 2] = [
    ("DKMQ", ShellFormulation::Dkmq),
    ("Rectangular", ShellFormulation::Rectangular),
];
const AXES: [(&str, Axis); 3] = [("X", Axis::X), ("Y", Axis::Y), ("Z", Axis::Z)];
fn labels<T>(items: &[(&str, T)]) -> Vec<SharedString> {
    items
        .iter()
        .map(|(l, _)| SharedString::from(l.to_string()))
        .collect()
}

/// Fields for one entity, in display order.
fn specs(model: &Model, id: EntityId) -> Option<(EntityKind, Vec<FieldSpec>)> {
    let kind = model.kind_of(id)?;
    let mut f = vec![];
    match kind {
        EntityKind::Node => {
            let n = &model.nodes[&id];
            f.push(text("name", "Name", &n.name));
            for (i, axis) in ["X", "Y", "Z"].iter().enumerate() {
                f.push(qty(
                    &format!("p{i}"),
                    axis,
                    Role::Length,
                    n.position[i].si(),
                ));
            }
            for (i, dof) in DOF.iter().enumerate() {
                f.push(check(format!("r{i}"), "Restraints", dof, n.restrained[i]));
            }
            for (i, axis) in ["kx", "ky", "kz"].iter().enumerate() {
                f.push(qty(
                    &format!("k{i}"),
                    axis,
                    Role::Stiffness,
                    n.spring_translation[i].si(),
                ));
            }
            for (i, axis) in ["Mass x", "Mass y", "Mass z"].iter().enumerate() {
                f.push(qty(&format!("m{i}"), axis, Role::Mass, n.mass[i].si()));
            }
        }
        EntityKind::Frame => {
            let e = &model.frames[&id];
            f.push(text("name", "Name", &e.name));
            f.push(entity_choice(
                "node_i",
                "Node I",
                model,
                EntityKind::Node,
                Some(e.nodes[0]),
            ));
            f.push(entity_choice(
                "node_j",
                "Node J",
                model,
                EntityKind::Node,
                Some(e.nodes[1]),
            ));
            f.push(entity_choice(
                "material",
                "Material",
                model,
                EntityKind::Material,
                Some(e.material),
            ));
            f.push(entity_choice(
                "section",
                "Section",
                model,
                EntityKind::Section,
                Some(e.section),
            ));
            let behavior = BEHAVIORS.iter().position(|(_, b)| *b == e.behavior);
            f.push(choice(
                "behavior",
                "Axial behavior",
                labels(&BEHAVIORS),
                behavior,
            ));
            f.push(qty("roll", "Roll", Role::Angle, e.roll.si()));
            for (i, dof) in DOF.iter().enumerate() {
                f.push(check(
                    format!("rel{i}"),
                    "Releases at I",
                    dof,
                    e.releases[i],
                ));
            }
            for (i, dof) in DOF.iter().enumerate() {
                f.push(check(
                    format!("rel{}", i + 6),
                    "Releases at J",
                    dof,
                    e.releases[i + 6],
                ));
            }
        }
        EntityKind::Shell => {
            let e = &model.shells[&id];
            f.push(text("name", "Name", &e.name));
            for i in 0..4 {
                f.push(entity_choice(
                    &format!("n{i}"),
                    &format!("Node {}", i + 1),
                    model,
                    EntityKind::Node,
                    Some(e.nodes[i]),
                ));
            }
            f.push(entity_choice(
                "material",
                "Material",
                model,
                EntityKind::Material,
                Some(e.material),
            ));
            f.push(qty(
                "thickness",
                "Thickness",
                Role::Thickness,
                e.thickness.si(),
            ));
            let formulation = FORMULATIONS.iter().position(|(_, x)| *x == e.formulation);
            f.push(choice(
                "formulation",
                "Formulation",
                labels(&FORMULATIONS),
                formulation,
            ));
        }
        EntityKind::Material => {
            let e = &model.materials[&id];
            f.push(text("name", "Name", &e.name));
            f.push(qty("young", "E", Role::Stress, e.young.si()));
            f.push(num("poisson", "Poisson's ratio", e.poisson));
            f.push(qty("density", "Density", Role::Density, e.density.si()));
        }
        EntityKind::Section => {
            let e = &model.sections[&id];
            f.push(text("name", "Name", &e.name));
            f.push(qty("area", "Area", Role::Area, e.area.si()));
            f.push(qty("iy", "Iy", Role::SecondMoment, e.iy.si()));
            f.push(qty("iz", "Iz", Role::SecondMoment, e.iz.si()));
            f.push(qty("torsion", "J", Role::SecondMoment, e.torsion.si()));
        }
        EntityKind::LoadCase => {
            let e = &model.load_cases[&id];
            f.push(text("name", "Name", &e.name));
            for (i, axis) in ["Self weight X", "Self weight Y", "Self weight Z"]
                .iter()
                .enumerate()
            {
                f.push(num(&format!("sw{i}"), axis, e.self_weight[i]));
            }
        }
        EntityKind::Combination => {
            let e = &model.combinations[&id];
            f.push(text("name", "Name", &e.name));
            for (i, (case, factor)) in e.terms.iter().enumerate() {
                f.push(entity_choice(
                    &format!("term_case_{i}"),
                    &format!("Case {}", i + 1),
                    model,
                    EntityKind::LoadCase,
                    Some(*case),
                ));
                f.push(num(
                    &format!("term_factor_{i}"),
                    &format!("Factor {}", i + 1),
                    *factor,
                ));
            }
        }
        EntityKind::Diaphragm => {
            let e = &model.diaphragms[&id];
            f.push(text("name", "Name", &e.name));
            let normal = AXES.iter().position(|(_, a)| *a == e.normal);
            f.push(choice("normal", "Normal axis", labels(&AXES), normal));
            let rows = rows_of(model, EntityKind::Node);
            let selected = e
                .master
                .and_then(|m| rows.iter().position(|(id, _)| *id == m))
                .map(|i| i + 1);
            let mut options: Vec<SharedString> = vec!["(automatic)".into()];
            options.extend(rows.into_iter().map(|(_, n)| SharedString::from(n)));
            f.push(choice(
                "master",
                "Master node",
                options,
                selected.or(Some(0)),
            ));
        }
        EntityKind::Group => {
            let e = &model.groups[&id];
            f.push(text("name", "Name", &e.name));
        }
    }
    Some((kind, f))
}

/// Values read back from the widgets, keyed like the specs.
#[derive(Default)]
struct Values {
    texts: HashMap<String, String>,
    checks: HashMap<String, bool>,
    choices: HashMap<String, Option<usize>>,
}
impl Values {
    fn text(&self, key: &str) -> String {
        self.texts.get(key).cloned().unwrap_or_default()
    }
    fn num(&self, key: &str, label: &str) -> Result<f64, String> {
        parse_num(label, &self.text(key))
    }
    /// A quantity typed in display units, as SI.
    fn qty(&self, key: &str, role: Role, label: &str) -> Result<f64, String> {
        parse_q(role, label, &self.text(key))
    }
    fn check(&self, key: &str) -> bool {
        self.checks.get(key).copied().unwrap_or(false)
    }
    fn choice(&self, key: &str) -> Option<usize> {
        self.choices.get(key).copied().flatten()
    }
    fn entity(
        &self,
        key: &str,
        label: &str,
        model: &Model,
        kind: EntityKind,
    ) -> Result<EntityId, String> {
        let rows = rows_of(model, kind);
        self.choice(key)
            .and_then(|i| rows.get(i))
            .map(|(id, _)| *id)
            .ok_or_else(|| format!("{label}: choose a {kind}"))
    }
    fn item<T: Copy>(&self, key: &str, items: &[(&str, T)], fallback: T) -> T {
        self.choice(key)
            .and_then(|i| items.get(i))
            .map(|(_, v)| *v)
            .unwrap_or(fallback)
    }
}

/// The command that writes the panel's values back to entity `id`, or None
/// when nothing changed.
fn command_for(
    model: &Model,
    id: EntityId,
    kind: EntityKind,
    v: &Values,
) -> Result<Option<Command>, String> {
    let name = v.text("name");
    Ok(match kind {
        EntityKind::Node => {
            let mut n = model.nodes[&id].clone();
            n.name = name;
            for i in 0..3 {
                n.position[i] =
                    Length::from_si(v.qty(&format!("p{i}"), Role::Length, "Position")?);
                n.spring_translation[i] =
                    Stiffness::from_si(v.qty(&format!("k{i}"), Role::Stiffness, "Spring")?);
                n.mass[i] = Mass::from_si(v.qty(&format!("m{i}"), Role::Mass, "Mass")?);
            }
            for i in 0..6 {
                n.restrained[i] = v.check(&format!("r{i}"));
            }
            (n != model.nodes[&id]).then_some(Command::UpdateNode { id, node: n })
        }
        EntityKind::Frame => {
            let mut e = model.frames[&id].clone();
            e.name = name;
            e.nodes = [
                v.entity("node_i", "Node I", model, EntityKind::Node)?,
                v.entity("node_j", "Node J", model, EntityKind::Node)?,
            ];
            e.material = v.entity("material", "Material", model, EntityKind::Material)?;
            e.section = v.entity("section", "Section", model, EntityKind::Section)?;
            e.behavior = v.item("behavior", &BEHAVIORS, e.behavior);
            e.roll = Angle::from_si(v.qty("roll", Role::Angle, "Roll")?);
            for i in 0..12 {
                e.releases[i] = v.check(&format!("rel{i}"));
            }
            (e != model.frames[&id]).then_some(Command::UpdateFrame { id, frame: e })
        }
        EntityKind::Shell => {
            let mut e = model.shells[&id].clone();
            e.name = name;
            for i in 0..4 {
                e.nodes[i] = v.entity(&format!("n{i}"), "Node", model, EntityKind::Node)?;
            }
            e.material = v.entity("material", "Material", model, EntityKind::Material)?;
            e.thickness = Length::from_si(v.qty("thickness", Role::Thickness, "Thickness")?);
            e.formulation = v.item("formulation", &FORMULATIONS, e.formulation);
            (e != model.shells[&id]).then_some(Command::UpdateShell { id, shell: e })
        }
        EntityKind::Material => {
            let mut e = model.materials[&id].clone();
            e.name = name;
            e.young = Pressure::from_si(v.qty("young", Role::Stress, "E")?);
            e.poisson = v.num("poisson", "Poisson's ratio")?;
            e.density = MassDensity::from_si(v.qty("density", Role::Density, "Density")?);
            (e != model.materials[&id]).then_some(Command::UpdateMaterial { id, material: e })
        }
        EntityKind::Section => {
            let mut e = model.sections[&id].clone();
            e.name = name;
            e.area = Area::from_si(v.qty("area", Role::Area, "Area")?);
            e.iy = SecondMoment::from_si(v.qty("iy", Role::SecondMoment, "Iy")?);
            e.iz = SecondMoment::from_si(v.qty("iz", Role::SecondMoment, "Iz")?);
            e.torsion = SecondMoment::from_si(v.qty("torsion", Role::SecondMoment, "J")?);
            (e != model.sections[&id]).then_some(Command::UpdateSection { id, section: e })
        }
        EntityKind::LoadCase => {
            let mut e = model.load_cases[&id].clone();
            e.name = name;
            for i in 0..3 {
                e.self_weight[i] = v.num(&format!("sw{i}"), "Self weight")?;
            }
            (e != model.load_cases[&id]).then_some(Command::UpdateLoadCase { id, load_case: e })
        }
        EntityKind::Combination => {
            let mut e = model.combinations[&id].clone();
            e.name = name;
            for i in 0..e.terms.len() {
                e.terms[i] = (
                    v.entity(
                        &format!("term_case_{i}"),
                        "Case",
                        model,
                        EntityKind::LoadCase,
                    )?,
                    v.num(&format!("term_factor_{i}"), "Factor")?,
                );
            }
            (e != model.combinations[&id])
                .then_some(Command::UpdateCombination { id, combination: e })
        }
        EntityKind::Diaphragm => {
            let mut e = model.diaphragms[&id].clone();
            e.name = name;
            e.normal = v.item("normal", &AXES, e.normal);
            let rows = rows_of(model, EntityKind::Node);
            e.master = v
                .choice("master")
                .filter(|i| *i > 0)
                .and_then(|i| rows.get(i - 1))
                .map(|(id, _)| *id);
            (e != model.diaphragms[&id]).then_some(Command::UpdateDiaphragm { id, diaphragm: e })
        }
        EntityKind::Group => {
            let mut e = model.groups[&id].clone();
            e.name = name;
            (e != model.groups[&id]).then_some(Command::UpdateGroup { id, group: e })
        }
    })
}

// MARK: Widgets

struct TextField {
    key: String,
    label: SharedString,
    input: Entity<InputState>,
    committed: String,
}
struct CheckField {
    key: String,
    group: SharedString,
    label: SharedString,
    value: bool,
}
struct ChoiceField {
    key: String,
    label: SharedString,
    select: Choice,
}

struct Single {
    id: EntityId,
    kind: EntityKind,
    texts: Vec<TextField>,
    checks: Vec<CheckField>,
    choices: Vec<ChoiceField>,
    _subscriptions: Vec<Subscription>,
}

struct Multi {
    section: Choice,
    material: Choice,
    restraints: [bool; 6],
    _subscriptions: Vec<Subscription>,
}

enum Shown {
    Nothing,
    Single(Single),
    Multi(Multi),
}

pub struct PropertyEditor {
    document: Entity<Document>,
    shown: Shown,
    /// Selection and revision the widgets were built for.
    built_for: (Vec<EntityId>, u64),
    /// Field to focus after the next rebuild, so Enter keeps the caret in place.
    pending_focus: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl PropertyEditor {
    pub fn new(document: Entity<Document>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe_in(&document, window, |this, _, window, cx| {
            this.sync(window, cx)
        });
        let mut this = Self {
            document,
            shown: Shown::Nothing,
            built_for: (vec![], u64::MAX),
            pending_focus: None,
            _subscriptions: vec![subscription],
        };
        this.sync(window, cx);
        this
    }

    /// Rebuilds the widgets when the selection or the model changed.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (selection, revision) = {
            let document = self.document.read(cx);
            (document.selection().to_vec(), document.revision())
        };
        if self.built_for == (selection.clone(), revision) {
            return;
        }
        self.built_for = (selection.clone(), revision);
        self.shown = match selection.as_slice() {
            [] => Shown::Nothing,
            [id] => self
                .build_single(*id, window, cx)
                .map_or(Shown::Nothing, Shown::Single),
            _ => Shown::Multi(self.build_multi(window, cx)),
        };
        if let Some(key) = self.pending_focus.take()
            && let Shown::Single(single) = &self.shown
            && let Some(field) = single.texts.iter().find(|t| t.key == key)
        {
            field.input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn build_single(
        &mut self,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Single> {
        let (kind, specs) = specs(self.document.read(cx).model(), id)?;
        let mut single = Single {
            id,
            kind,
            texts: vec![],
            checks: vec![],
            choices: vec![],
            _subscriptions: vec![],
        };
        for spec in specs {
            match spec {
                FieldSpec::Text { key, label, value } => {
                    let input =
                        cx.new(|cx| InputState::new(window, cx).default_value(value.clone()));
                    let field_key = key.clone();
                    single._subscriptions.push(cx.subscribe_in(
                        &input,
                        window,
                        move |this, _, event: &InputEvent, window, cx| match event {
                            InputEvent::PressEnter { .. } => {
                                this.pending_focus = Some(field_key.clone());
                                this.commit(window, cx);
                            }
                            InputEvent::Blur => this.commit(window, cx),
                            _ => {}
                        },
                    ));
                    single.texts.push(TextField {
                        key,
                        label,
                        input,
                        committed: value,
                    });
                }
                FieldSpec::Check {
                    key,
                    group,
                    label,
                    value,
                } => {
                    single.checks.push(CheckField {
                        key,
                        group,
                        label,
                        value,
                    });
                }
                FieldSpec::Choice {
                    key,
                    label,
                    options,
                    selected,
                } => {
                    let select = cx.new(|cx| {
                        SelectState::new(
                            SearchableVec::from(options),
                            selected.map(IndexPath::new),
                            window,
                            cx,
                        )
                    });
                    single._subscriptions.push(cx.subscribe_in(
                        &select,
                        window,
                        |this, _, _: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                            this.commit(window, cx)
                        },
                    ));
                    single.choices.push(ChoiceField { key, label, select });
                }
            }
        }
        Some(single)
    }

    fn build_multi(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Multi {
        let model = self.document.read(cx).model();
        let sections: Vec<SharedString> = rows_of(model, EntityKind::Section)
            .into_iter()
            .map(|(_, n)| n.into())
            .collect();
        let materials: Vec<SharedString> = rows_of(model, EntityKind::Material)
            .into_iter()
            .map(|(_, n)| n.into())
            .collect();
        let section =
            cx.new(|cx| SelectState::new(SearchableVec::from(sections), None, window, cx));
        let material =
            cx.new(|cx| SelectState::new(SearchableVec::from(materials), None, window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &section,
                window,
                |this, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    let SelectEvent::Confirm(Some(name)) = event else {
                        return;
                    };
                    this.assign_to_frames(EntityKind::Section, name.clone(), window, cx);
                },
            ),
            cx.subscribe_in(
                &material,
                window,
                |this, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    let SelectEvent::Confirm(Some(name)) = event else {
                        return;
                    };
                    this.assign_to_frames(EntityKind::Material, name.clone(), window, cx);
                },
            ),
        ];
        Multi {
            section,
            material,
            restraints: [false; 6],
            _subscriptions: subscriptions,
        }
    }

    fn report(&self, result: Result<(), String>, window: &mut Window, cx: &mut App) {
        if let Err(message) = result {
            window.push_notification(Notification::error(message), cx);
        }
    }

    /// Reads every widget and applies the resulting update command.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Shown::Single(single) = &self.shown else {
            return;
        };
        let mut values = Values::default();
        for t in &single.texts {
            values
                .texts
                .insert(t.key.clone(), t.input.read(cx).value().to_string());
        }
        for c in &single.checks {
            values.checks.insert(c.key.clone(), c.value);
        }
        for c in &single.choices {
            values.choices.insert(
                c.key.clone(),
                c.select.read(cx).selected_index(cx).map(|ix| ix.row),
            );
        }
        let (id, kind) = (single.id, single.kind);
        let unchanged = single
            .texts
            .iter()
            .all(|t| values.text(&t.key) == t.committed);
        let result = {
            let document = self.document.read(cx);
            command_for(document.model(), id, kind, &values)
        };
        let result = match result {
            Ok(None) => Ok(()),
            Ok(Some(command)) => self
                .document
                .update(cx, |document, cx| document.apply(command, cx))
                .map_err(|e| e.to_string()),
            Err(message) => {
                if unchanged {
                    // A blur with unparsable but untouched text is not worth a message.
                    Ok(())
                } else {
                    Err(message)
                }
            }
        };
        self.report(result, window, cx);
    }

    fn set_check(&mut self, key: String, value: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Shown::Single(single) = &mut self.shown
            && let Some(field) = single.checks.iter_mut().find(|c| c.key == key)
        {
            field.value = value;
            self.commit(window, cx);
        }
    }

    fn apply(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let result = self
            .document
            .update(cx, |document, cx| document.apply(command, cx))
            .map_err(|e| e.to_string());
        self.report(result, window, cx);
    }

    fn assign_to_frames(
        &mut self,
        kind: EntityKind,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = {
            let document = self.document.read(cx);
            let model = document.model();
            let Some(target) = rows_of(model, kind)
                .into_iter()
                .find(|(_, n)| *n == name.as_ref())
                .map(|(id, _)| id)
            else {
                return;
            };
            document
                .selected_of(EntityKind::Frame)
                .into_iter()
                .map(|id| {
                    let mut frame = model.frames[&id].clone();
                    match kind {
                        EntityKind::Section => frame.section = target,
                        _ => frame.material = target,
                    }
                    Command::UpdateFrame { id, frame }
                })
                .collect::<Vec<_>>()
        };
        if !commands.is_empty() {
            self.apply(Command::Batch { commands }, window, cx);
        }
    }

    fn assign_restraints(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Shown::Multi(multi) = &self.shown else {
            return;
        };
        let restrained = multi.restraints;
        let commands = {
            let document = self.document.read(cx);
            document
                .selected_of(EntityKind::Node)
                .into_iter()
                .map(|id| {
                    let mut node = document.model().nodes[&id].clone();
                    node.restrained = restrained;
                    Command::UpdateNode { id, node }
                })
                .collect::<Vec<_>>()
        };
        if !commands.is_empty() {
            self.apply(Command::Batch { commands }, window, cx);
        }
    }

    fn remove_load(
        &mut self,
        id: EntityId,
        which: LoadRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut load_case = self.document.read(cx).model().load_cases[&id].clone();
        match which {
            LoadRef::Nodal(i) => {
                load_case.nodal.remove(i);
            }
            LoadRef::Member(i) => {
                load_case.member.remove(i);
            }
            LoadRef::Surface(i) => {
                load_case.surface.remove(i);
            }
        }
        self.apply(Command::UpdateLoadCase { id, load_case }, window, cx);
    }

    fn add_term(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.document.read(cx).model();
        let Some(case) = model.load_cases.keys().next().copied() else {
            self.report(Err("Define a load case first".into()), window, cx);
            return;
        };
        let mut combination = model.combinations[&id].clone();
        combination.terms.push((case, 1.0));
        self.apply(Command::UpdateCombination { id, combination }, window, cx);
    }
    fn remove_term(
        &mut self,
        id: EntityId,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut combination = self.document.read(cx).model().combinations[&id].clone();
        combination.terms.remove(ix);
        self.apply(Command::UpdateCombination { id, combination }, window, cx);
    }

    fn set_diaphragm_nodes(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.document.read(cx);
        let mut diaphragm = document.model().diaphragms[&id].clone();
        diaphragm.nodes = document.selected_of(EntityKind::Node);
        self.apply(Command::UpdateDiaphragm { id, diaphragm }, window, cx);
    }

    fn set_group_members(
        &mut self,
        id: EntityId,
        add: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let document = self.document.read(cx);
        let mut group = document.model().groups[&id].clone();
        if !add {
            group.members.clear();
        }
        group
            .members
            .extend(document.selection().iter().copied().filter(|m| *m != id));
        self.apply(Command::UpdateGroup { id, group }, window, cx);
    }
    fn select_group_members(&mut self, id: EntityId, cx: &mut Context<Self>) {
        self.document.update(cx, |document, cx| {
            let members = document.model().group_members(id);
            document.set_selection(members, cx);
        });
    }

    // MARK: Rendering

    fn render_single(
        &self,
        single: &Single,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut form = Form::new().label_text_size(rems(0.75));
        for t in &single.texts {
            form = form.child(
                Field::new()
                    .label(t.label.clone())
                    .child(Input::new(&t.input).small()),
            );
        }
        for c in &single.choices {
            form = form.child(
                Field::new()
                    .label(c.label.clone())
                    .child(Select::new(&c.select).small()),
            );
        }
        let mut groups: Vec<SharedString> = vec![];
        for c in &single.checks {
            if !groups.contains(&c.group) {
                groups.push(c.group.clone());
            }
        }
        for group in groups {
            let boxes = single.checks.iter().filter(|c| c.group == group).map(|c| {
                let key = c.key.clone();
                Checkbox::new(SharedString::from(format!("check-{}", c.key)))
                    .small()
                    .label(c.label.clone())
                    .checked(c.value)
                    .on_change(cx.listener(move |this, value: &bool, window, cx| {
                        this.set_check(key.clone(), *value, window, cx)
                    }))
            });
            form = form.child(
                Field::new()
                    .label(group.clone())
                    .child(h_flex().flex_wrap().gap_x_3().gap_y_1().children(boxes)),
            );
        }
        let extras = self.render_extras(single, window, cx);
        v_flex()
            .gap_3()
            .child(form)
            .children(extras)
            .child(
                h_flex().gap_2().child(
                    Button::new("delete-entity")
                        .small()
                        .danger()
                        .label("Delete")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(DeleteSelected), cx)
                        }),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("id #{}", single.id.0)),
            )
            .into_any_element()
    }

    fn render_extras(
        &self,
        single: &Single,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = single.id;
        let document = self.document.read(cx);
        let model = document.model();
        let muted = cx.theme().muted_foreground;
        let row = |label: String, remove: AnyElement| {
            h_flex()
                .gap_2()
                .text_xs()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(label),
                )
                .child(remove)
        };
        Some(match single.kind {
            EntityKind::LoadCase => {
                let case = &model.load_cases[&id];
                let mut list = v_flex().gap_1();
                for (i, l) in case.nodal.iter().enumerate() {
                    let label = format!(
                        "{}: F ({}, {}, {}) {}  M ({}, {}, {}) {}",
                        model.name_of(l.node).unwrap_or("?"),
                        fmt_q(Role::Force, l.force[0].si()),
                        fmt_q(Role::Force, l.force[1].si()),
                        fmt_q(Role::Force, l.force[2].si()),
                        UNITS.symbol(Role::Force),
                        fmt_q(Role::Moment, l.moment[0].si()),
                        fmt_q(Role::Moment, l.moment[1].si()),
                        fmt_q(Role::Moment, l.moment[2].si()),
                        UNITS.symbol(Role::Moment),
                    );
                    list = list.child(row(
                        label,
                        remove_button(
                            ("remove-nodal", i),
                            cx.listener(move |this, _, window, cx| {
                                this.remove_load(id, LoadRef::Nodal(i), window, cx)
                            }),
                        ),
                    ));
                }
                for (i, l) in case.member.iter().enumerate() {
                    let label = match l {
                        MemberLoad::Point {
                            member,
                            position,
                            force,
                            axes,
                            ..
                        } => format!(
                            "{}: point at {} {}, F ({}, {}, {}) {}, {:?}",
                            model.name_of(*member).unwrap_or("?"),
                            fmt_q(Role::Length, position.si()),
                            UNITS.symbol(Role::Length),
                            fmt_q(Role::Force, force[0].si()),
                            fmt_q(Role::Force, force[1].si()),
                            fmt_q(Role::Force, force[2].si()),
                            UNITS.symbol(Role::Force),
                            axes
                        ),
                        MemberLoad::Distributed {
                            member,
                            start,
                            end,
                            start_load,
                            end_load,
                            axes,
                        } => format!(
                            "{}: distributed {}–{} {}, w ({}, {}, {}) → ({}, {}, {}) {}, {:?}",
                            model.name_of(*member).unwrap_or("?"),
                            fmt_q(Role::Length, start.si()),
                            fmt_q(Role::Length, end.si()),
                            UNITS.symbol(Role::Length),
                            fmt_q(Role::LineLoad, start_load[0].si()),
                            fmt_q(Role::LineLoad, start_load[1].si()),
                            fmt_q(Role::LineLoad, start_load[2].si()),
                            fmt_q(Role::LineLoad, end_load[0].si()),
                            fmt_q(Role::LineLoad, end_load[1].si()),
                            fmt_q(Role::LineLoad, end_load[2].si()),
                            UNITS.symbol(Role::LineLoad),
                            axes
                        ),
                    };
                    list = list.child(row(
                        label,
                        remove_button(
                            ("remove-member", i),
                            cx.listener(move |this, _, window, cx| {
                                this.remove_load(id, LoadRef::Member(i), window, cx)
                            }),
                        ),
                    ));
                }
                for (i, l) in case.surface.iter().enumerate() {
                    let label = format!(
                        "{}: pressure {} {}",
                        model.name_of(l.shell).unwrap_or("?"),
                        fmt_q(Role::Pressure, l.pressure.si()),
                        UNITS.symbol(Role::Pressure),
                    );
                    list = list.child(row(
                        label,
                        remove_button(
                            ("remove-surface", i),
                            cx.listener(move |this, _, window, cx| {
                                this.remove_load(id, LoadRef::Surface(i), window, cx)
                            }),
                        ),
                    ));
                }
                let total = case.nodal.len() + case.member.len() + case.surface.len();
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("Loads ({total})")),
                    )
                    .child(list)
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new("add-nodal-load")
                                    .small()
                                    .outline()
                                    .label("Add nodal load…")
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(AddNodalLoad), cx)
                                    }),
                            )
                            .child(
                                Button::new("add-distributed-load")
                                    .small()
                                    .outline()
                                    .label("Add distributed load…")
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(AddDistributedLoad), cx)
                                    }),
                            ),
                    )
                    .into_any_element()
            }
            EntityKind::Combination => {
                let terms = model.combinations[&id].terms.len();
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("add-term")
                            .small()
                            .outline()
                            .label("Add term")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_term(id, window, cx)
                            })),
                    )
                    .children((0..terms).map(|i| {
                        Button::new(("remove-term", i))
                            .small()
                            .danger()
                            .outline()
                            .label(format!("Remove term {}", i + 1))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_term(id, i, window, cx)
                            }))
                    }))
                    .into_any_element()
            }
            EntityKind::Diaphragm => {
                let count = model.diaphragms[&id].nodes.len();
                let selected = document.selected_of(EntityKind::Node).len();
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("{count} constrained nodes")),
                    )
                    .child(
                        Button::new("set-diaphragm-nodes")
                            .small()
                            .outline()
                            .label(format!("Set nodes from selection ({selected})"))
                            .disabled(selected == 0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_diaphragm_nodes(id, window, cx)
                            })),
                    )
                    .into_any_element()
            }
            EntityKind::Group => {
                let count = model.group_members(id).len();
                let selected = document.selection().len().saturating_sub(1);
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("{count} members")),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new("group-set")
                                    .small()
                                    .outline()
                                    .label(format!("Set from selection ({selected})"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_group_members(id, false, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("group-add")
                                    .small()
                                    .outline()
                                    .label("Add selection")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_group_members(id, true, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("group-select")
                                    .small()
                                    .outline()
                                    .label("Select members")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_group_members(id, cx)
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            _ => return None,
        })
    }

    fn render_multi(&self, multi: &Multi, cx: &mut Context<Self>) -> AnyElement {
        let document = self.document.read(cx);
        let muted = cx.theme().muted_foreground;
        let mut counts: Vec<(EntityKind, usize)> = vec![];
        for id in document.selection() {
            if let Some(kind) = document.model().kind_of(*id) {
                match counts.iter_mut().find(|(k, _)| *k == kind) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((kind, 1)),
                }
            }
        }
        let summary = counts
            .iter()
            .map(|(k, n)| format!("{n} {k}{}", if *n == 1 { "" } else { "s" }))
            .collect::<Vec<_>>()
            .join(", ");
        let frames = document.selected_of(EntityKind::Frame).len();
        let nodes = document.selected_of(EntityKind::Node).len();
        let restraints = multi.restraints;
        v_flex()
            .gap_3()
            .child(div().text_sm().child(summary))
            .when(frames > 0, |this| {
                this.child(
                    Form::new()
                        .label_text_size(rems(0.75))
                        .child(
                            Field::new()
                                .label(format!("Assign section to {frames} frames"))
                                .child(
                                    Select::new(&multi.section)
                                        .small()
                                        .placeholder("Choose a section"),
                                ),
                        )
                        .child(
                            Field::new()
                                .label(format!("Assign material to {frames} frames"))
                                .child(
                                    Select::new(&multi.material)
                                        .small()
                                        .placeholder("Choose a material"),
                                ),
                        ),
                )
            })
            .when(nodes > 0, |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("Restraints for {nodes} nodes")),
                        )
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap_x_3()
                                .gap_y_1()
                                .children((0..6).map(|i| {
                                    Checkbox::new(("multi-restraint", i))
                                        .small()
                                        .label(DOF[i])
                                        .checked(restraints[i])
                                        .on_change(cx.listener(move |this, value: &bool, _, cx| {
                                            if let Shown::Multi(multi) = &mut this.shown {
                                                multi.restraints[i] = *value;
                                                cx.notify();
                                            }
                                        }))
                                })),
                        )
                        .child(
                            Button::new("apply-restraints")
                                .small()
                                .outline()
                                .label("Apply restraints")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.assign_restraints(window, cx)
                                })),
                        ),
                )
            })
            .child(
                Button::new("delete-selected")
                    .small()
                    .danger()
                    .label("Delete selected")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(DeleteSelected), cx)),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum LoadRef {
    Nodal(usize),
    Member(usize),
    Surface(usize),
}

fn remove_button(
    id: impl Into<ElementId>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    Button::new(id)
        .xsmall()
        .danger()
        .outline()
        .label("Remove")
        .on_click(on_click)
        .into_any_element()
}

impl Render for PropertyEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg, muted) = (
            theme.sidebar,
            theme.sidebar_foreground,
            theme.muted_foreground,
        );
        let title: SharedString = match &self.shown {
            Shown::Nothing => "Properties".into(),
            Shown::Single(single) => {
                let name = self
                    .document
                    .read(cx)
                    .model()
                    .name_of(single.id)
                    .unwrap_or("?");
                format!("{} {name}", capitalize(&single.kind.to_string())).into()
            }
            Shown::Multi(_) => "Selection".into(),
        };
        let body = match &self.shown {
            Shown::Nothing => div()
                .text_sm()
                .text_color(muted)
                .child("Select an entity in the view or the model tree to edit it.")
                .into_any_element(),
            Shown::Single(single) => self.render_single(single, window, cx),
            Shown::Multi(multi) => self.render_multi(multi, cx),
        };
        v_flex()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                v_flex()
                    .id("properties-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .px_3()
                    .pb_3()
                    .child(body),
            )
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
