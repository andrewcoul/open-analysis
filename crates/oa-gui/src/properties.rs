//! The property panel: edits the selected entity through inverse-returning
//! commands. Text fields commit on Enter or blur, checkboxes and choices
//! commit at once. With several entities selected it offers bulk assignment.
use crate::actions::{AddDistributedLoad, AddNodalLoad, DeleteSelected, SetActiveLevel};
use crate::document::Document;
use crate::explorer::rows_of;
use crate::prompt::selection_summary;
use crate::results::{Plane, render_member_results};
use crate::text::{UNITS, fmt_num, fmt_q, label, parse_num, parse_opt_q, parse_q};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use oa_core::units::Length;
use oa_core::units::*;
use oa_model::{
    AxialBehavior, Axis, Command, ElevationScope, EntityId, EntityKind, FrameModifiers, Level,
    MemberLoad, Model, Role, ShellFormulation, ShellModifiers,
};
use std::collections::HashMap;

type Choice = Entity<SelectState<SearchableVec<SharedString>>>;

/// The two tabs a frame shows once it has results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EditorTab {
    #[default]
    Properties,
    Results,
}

/// The form is a grid of six columns, so a field can take the whole row,
/// half of it (Node I beside Node J), a third (X, Y, Z), or two thirds.
const COLUMNS: usize = 6;
const FULL: u16 = 6;
const TWO_THIRDS: u16 = 4;
const HALF: u16 = 3;
const THIRD: u16 = 2;

/// What one field shows, independent of the widget that edits it. Fields
/// are laid out in the order they are listed.
enum FieldSpec {
    Text {
        key: String,
        label: SharedString,
        value: String,
        span: u16,
    },
    /// Checkboxes listed one after another under the same group share a row.
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
        span: u16,
    },
}
impl FieldSpec {
    /// Columns of the grid the field takes; the full row unless set.
    fn span(mut self, columns: u16) -> Self {
        match &mut self {
            FieldSpec::Text { span, .. } | FieldSpec::Choice { span, .. } => *span = columns,
            FieldSpec::Check { .. } => {}
        }
        self
    }
}
fn text(key: &str, label: &str, value: impl Into<String>) -> FieldSpec {
    FieldSpec::Text {
        key: key.into(),
        label: label.into(),
        value: value.into(),
        span: FULL,
    }
}
fn num(key: &str, label: &str, value: f64) -> FieldSpec {
    text(key, label, fmt_num(value))
}
/// A quantity field: labelled with its unit, showing the SI value converted.
fn qty(key: &str, name: &str, role: Role, si: f64) -> FieldSpec {
    text(key, &label(name, role), fmt_q(role, si))
}
/// A quantity field that may be blank, meaning none.
fn opt_qty(key: &str, name: &str, role: Role, si: Option<f64>) -> FieldSpec {
    text(
        key,
        &label(name, role),
        si.map_or(String::new(), |v| fmt_q(role, v)),
    )
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
        span: FULL,
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
/// A shell's local x reference, projected into its plane.
const SHELL_LOCAL_X: [(&str, Option<[f64; 3]>); 4] = [
    ("Along first edge", None),
    ("Global X", Some([1.0, 0.0, 0.0])),
    ("Global Y", Some([0.0, 1.0, 0.0])),
    ("Global Z", Some([0.0, 0.0, 1.0])),
];
/// Cracked-section stiffness from ACI 318 Table 6.6.3.1.1(a), assigned to
/// every selected frame or shell at once. Each preset replaces a member's
/// stiffness modifiers and keeps its mass and weight modifiers.
const fn cracked_frame(i: f64) -> FrameModifiers {
    FrameModifiers {
        area: 1.0,
        shear_y: 1.0,
        shear_z: 1.0,
        torsion: 1.0,
        iy: i,
        iz: i,
        mass: 1.0,
        weight: 1.0,
    }
}
const FRAME_PRESETS: [(&str, FrameModifiers); 3] = [
    ("Full stiffness", cracked_frame(1.0)),
    ("ACI 318 beam: 0.35 Ig", cracked_frame(0.35)),
    ("ACI 318 column: 0.70 Ig", cracked_frame(0.7)),
];
const fn cracked_shell(membrane: f64, bending: f64) -> ShellModifiers {
    ShellModifiers {
        membrane_x: membrane,
        membrane_y: membrane,
        membrane_shear: 1.0,
        bending,
        mass: 1.0,
        weight: 1.0,
    }
}
const SHELL_PRESETS: [(&str, ShellModifiers); 4] = [
    ("Full stiffness", cracked_shell(1.0, 1.0)),
    ("ACI 318 wall, uncracked: 0.70 Ig", cracked_shell(0.7, 0.7)),
    ("ACI 318 wall, cracked: 0.35 Ig", cracked_shell(0.35, 0.35)),
    ("ACI 318 flat slab: 0.25 Ig", cracked_shell(1.0, 0.25)),
];
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
        EntityKind::Level => {
            let l = &model.levels[&id];
            f.push(text("name", "Name", &l.name).span(HALF));
            f.push(qty("elevation", "Elevation", Role::Length, l.elevation.si()).span(HALF));
        }
        EntityKind::Node => {
            let n = &model.nodes[&id];
            f.push(text("name", "Name", &n.name));
            for (i, axis) in ["X", "Y", "Z"].iter().enumerate() {
                f.push(
                    qty(&format!("p{i}"), axis, Role::Length, n.position[i].si()).span(THIRD),
                );
            }
            f.push(
                entity_choice("level", "Level", model, EntityKind::Level, Some(n.level))
                    .span(HALF),
            );
            let offset = model
                .levels
                .get(&n.level)
                .map_or(0.0, |l| oa_model::levels::offset(n, l));
            f.push(qty("offset", "Offset above level", Role::Length, offset).span(HALF));
            for (i, dof) in DOF.iter().enumerate() {
                f.push(check(format!("r{i}"), "Restraints", dof, n.restrained[i]));
            }
            for (i, axis) in ["Spring kx", "Spring ky", "Spring kz"].iter().enumerate() {
                f.push(
                    qty(
                        &format!("k{i}"),
                        axis,
                        Role::Stiffness,
                        n.spring_translation[i].si(),
                    )
                    .span(THIRD),
                );
            }
            for (i, axis) in ["Mass x", "Mass y", "Mass z"].iter().enumerate() {
                f.push(qty(&format!("m{i}"), axis, Role::Mass, n.mass[i].si()).span(THIRD));
            }
        }
        EntityKind::Frame => {
            let e = &model.frames[&id];
            f.push(text("name", "Name", &e.name));
            f.push(
                entity_choice("node_i", "Node I", model, EntityKind::Node, Some(e.nodes[0]))
                    .span(HALF),
            );
            f.push(
                entity_choice("node_j", "Node J", model, EntityKind::Node, Some(e.nodes[1]))
                    .span(HALF),
            );
            f.push(
                entity_choice(
                    "material",
                    "Material",
                    model,
                    EntityKind::Material,
                    Some(e.material),
                )
                .span(HALF),
            );
            f.push(
                entity_choice(
                    "section",
                    "Section",
                    model,
                    EntityKind::Section,
                    Some(e.section),
                )
                .span(HALF),
            );
            let behavior = BEHAVIORS.iter().position(|(_, b)| *b == e.behavior);
            f.push(
                choice("behavior", "Axial behavior", labels(&BEHAVIORS), behavior)
                    .span(TWO_THIRDS),
            );
            f.push(qty("roll", "Roll", Role::Angle, e.roll.si()).span(THIRD));
            let md = &e.modifiers;
            for (key, name, value) in [
                ("mod_iy", "Iy modifier", md.iy),
                ("mod_iz", "Iz modifier", md.iz),
                ("mod_torsion", "J modifier", md.torsion),
                ("mod_area", "A modifier", md.area),
                ("mod_shear_y", "Shear y modifier", md.shear_y),
                ("mod_shear_z", "Shear z modifier", md.shear_z),
            ] {
                f.push(num(key, name, value).span(THIRD));
            }
            f.push(num("mod_mass", "Mass modifier", md.mass).span(HALF));
            f.push(num("mod_weight", "Weight modifier", md.weight).span(HALF));
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
                f.push(
                    entity_choice(
                        &format!("n{i}"),
                        &format!("Node {}", i + 1),
                        model,
                        EntityKind::Node,
                        Some(e.nodes[i]),
                    )
                    .span(HALF),
                );
            }
            f.push(
                entity_choice(
                    "material",
                    "Material",
                    model,
                    EntityKind::Material,
                    Some(e.material),
                )
                .span(HALF),
            );
            f.push(qty("thickness", "Thickness", Role::Thickness, e.thickness.si()).span(HALF));
            let formulation = FORMULATIONS.iter().position(|(_, x)| *x == e.formulation);
            f.push(
                choice("formulation", "Formulation", labels(&FORMULATIONS), formulation)
                    .span(HALF),
            );
            let local_x = SHELL_LOCAL_X.iter().position(|(_, x)| *x == e.local_x);
            f.push(choice("local_x", "Local x", labels(&SHELL_LOCAL_X), local_x).span(HALF));
            let md = &e.modifiers;
            for (key, name, value) in [
                ("mod_membrane_x", "Membrane x modifier", md.membrane_x),
                ("mod_membrane_y", "Membrane y modifier", md.membrane_y),
                (
                    "mod_membrane_shear",
                    "In-plane shear modifier",
                    md.membrane_shear,
                ),
                ("mod_bending", "Bending modifier", md.bending),
                ("mod_mass", "Mass modifier", md.mass),
                ("mod_weight", "Weight modifier", md.weight),
            ] {
                f.push(num(key, name, value).span(HALF));
            }
        }
        EntityKind::Material => {
            let e = &model.materials[&id];
            f.push(text("name", "Name", &e.name));
            f.push(qty("young", "E", Role::Stress, e.young.si()).span(THIRD));
            f.push(num("poisson", "Poisson's ratio", e.poisson).span(THIRD));
            f.push(qty("density", "Density", Role::Density, e.density.si()).span(THIRD));
            f.push(opt_qty("fy", "Fy", Role::Stress, e.fy.map(Pressure::si)).span(THIRD));
            f.push(opt_qty("fu", "Fu", Role::Stress, e.fu.map(Pressure::si)).span(THIRD));
            f.push(opt_qty("fc", "f'c", Role::Stress, e.fc.map(Pressure::si)).span(THIRD));
        }
        EntityKind::Section => {
            let e = &model.sections[&id];
            f.push(text("name", "Name", &e.name));
            f.push(qty("area", "Area", Role::Area, e.area.si()).span(HALF));
            f.push(qty("torsion", "J", Role::SecondMoment, e.torsion.si()).span(HALF));
            f.push(qty("iy", "Iy", Role::SecondMoment, e.iy.si()).span(HALF));
            f.push(qty("iz", "Iz", Role::SecondMoment, e.iz.si()).span(HALF));
            for (key, name, shear) in [
                ("shear_y", "Shear area y", e.shear_y),
                ("shear_z", "Shear area z", e.shear_z),
            ] {
                f.push(opt_qty(key, name, Role::Area, shear.map(Area::si)).span(HALF));
            }
        }
        EntityKind::LoadCase => {
            let e = &model.load_cases[&id];
            f.push(text("name", "Name", &e.name));
            for (i, axis) in ["Self weight X", "Self weight Y", "Self weight Z"]
                .iter()
                .enumerate()
            {
                f.push(num(&format!("sw{i}"), axis, e.self_weight[i]).span(THIRD));
            }
        }
        EntityKind::Combination => {
            let e = &model.combinations[&id];
            f.push(text("name", "Name", &e.name));
            for (i, (case, factor)) in e.terms.iter().enumerate() {
                f.push(
                    entity_choice(
                        &format!("term_case_{i}"),
                        &format!("Case {}", i + 1),
                        model,
                        EntityKind::LoadCase,
                        Some(*case),
                    )
                    .span(TWO_THIRDS),
                );
                f.push(
                    num(&format!("term_factor_{i}"), &format!("Factor {}", i + 1), *factor)
                        .span(THIRD),
                );
            }
        }
        EntityKind::Diaphragm => {
            let e = &model.diaphragms[&id];
            f.push(text("name", "Name", &e.name));
            let normal = AXES.iter().position(|(_, a)| *a == e.normal);
            f.push(choice("normal", "Normal axis", labels(&AXES), normal).span(THIRD));
            let rows = rows_of(model, EntityKind::Node);
            let selected = e
                .master
                .and_then(|m| rows.iter().position(|(id, _)| *id == m))
                .map(|i| i + 1);
            let mut options: Vec<SharedString> = vec!["(automatic)".into()];
            options.extend(rows.into_iter().map(|(_, n)| SharedString::from(n)));
            f.push(choice("master", "Master node", options, selected.or(Some(0))).span(TWO_THIRDS));
        }
        EntityKind::Group => {
            let e = &model.groups[&id];
            f.push(text("name", "Name", &e.name));
        }
        EntityKind::MassSource => {
            // Load case multipliers are edited in the mass sources table.
            let e = &model.mass_sources[&id];
            f.push(text("name", "Name", &e.name));
            for (key, label, value) in [
                ("element_mass", "Members' own mass", e.element_mass),
                ("lateral", "Lateral mass", e.lateral),
                ("vertical", "Vertical mass", e.vertical),
                ("lump", "Lump lateral mass to levels", e.lump_to_levels),
            ] {
                f.push(check(key.into(), "Mass", label, value));
            }
        }
        EntityKind::Underlay => {
            let e = &model.underlays[&id];
            f.push(text("name", "Name", &e.name));
            f.push(entity_choice(
                "level",
                "Level",
                model,
                EntityKind::Level,
                Some(e.level),
            ));
            for (i, axis) in ["Origin X", "Origin Y"].iter().enumerate() {
                f.push(qty(&format!("o{i}"), axis, Role::Length, e.origin[i].si()).span(HALF));
            }
        }
    }
    Some((kind, f))
}

/// Values read back from the widgets, keyed like the specs.
#[derive(Default)]
struct Values {
    texts: HashMap<String, String>,
    /// What each text field showed when it was built. Display text is
    /// rounded, so a field the user did not touch keeps its stored value
    /// rather than the rounded text parsed back.
    committed: HashMap<String, String>,
    checks: HashMap<String, bool>,
    choices: HashMap<String, Option<usize>>,
}
impl Values {
    fn text(&self, key: &str) -> String {
        self.texts.get(key).cloned().unwrap_or_default()
    }
    fn changed(&self, key: &str) -> bool {
        self.texts.get(key) != self.committed.get(key)
    }
    /// A plain number, or `current` when the field was not edited.
    fn num(&self, key: &str, label: &str, current: f64) -> Result<f64, String> {
        if !self.changed(key) {
            return Ok(current);
        }
        parse_num(label, &self.text(key))
    }
    /// A quantity typed in display units, as SI, or `current` (SI) when the
    /// field was not edited.
    fn qty(&self, key: &str, role: Role, label: &str, current: f64) -> Result<f64, String> {
        if !self.changed(key) {
            return Ok(current);
        }
        parse_q(role, label, &self.text(key))
    }
    /// A quantity that may be blank, as SI or none, or `current` when the
    /// field was not edited.
    fn opt_qty(
        &self,
        key: &str,
        role: Role,
        label: &str,
        current: Option<f64>,
    ) -> Result<Option<f64>, String> {
        if !self.changed(key) {
            return Ok(current);
        }
        parse_opt_q(role, label, &self.text(key))
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

/// Updates giving each of `frames` the modifiers of the preset labelled
/// `name`, leaving out frames that already have them.
fn frame_modifier_commands(model: &Model, frames: &[EntityId], name: &str) -> Vec<Command> {
    let Some((_, modifiers)) = FRAME_PRESETS.iter().find(|(label, _)| *label == name) else {
        return vec![];
    };
    frames
        .iter()
        .filter_map(|&id| {
            let mut frame = model.frames[&id].clone();
            frame.modifiers = FrameModifiers {
                mass: frame.modifiers.mass,
                weight: frame.modifiers.weight,
                ..*modifiers
            };
            (frame != model.frames[&id]).then_some(Command::UpdateFrame { id, frame })
        })
        .collect()
}

/// Updates giving each of `shells` the modifiers of the preset labelled
/// `name`, leaving out shells that already have them.
fn shell_modifier_commands(model: &Model, shells: &[EntityId], name: &str) -> Vec<Command> {
    let Some((_, modifiers)) = SHELL_PRESETS.iter().find(|(label, _)| *label == name) else {
        return vec![];
    };
    shells
        .iter()
        .filter_map(|&id| {
            let mut shell = model.shells[&id].clone();
            shell.modifiers = ShellModifiers {
                mass: shell.modifiers.mass,
                weight: shell.modifiers.weight,
                ..*modifiers
            };
            (shell != model.shells[&id]).then_some(Command::UpdateShell { id, shell })
        })
        .collect()
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
        EntityKind::Level => {
            let current = &model.levels[&id];
            let mut commands = vec![];
            if name != current.name {
                commands.push(Command::UpdateLevel {
                    id,
                    level: Level {
                        name,
                        elevation: current.elevation,
                    },
                });
            }
            let elevation = v.qty("elevation", Role::Length, "Elevation", current.elevation.si())?;
            if elevation != current.elevation.si() {
                // The level moves with its nodes; the other levels stay.
                commands.push(Command::SetLevelElevation {
                    id,
                    elevation: Length::from_si(elevation),
                    scope: ElevationScope::ThisLevel,
                });
            }
            match commands.len() {
                0 => None,
                1 => commands.pop(),
                _ => Some(Command::Batch { commands }),
            }
        }
        EntityKind::Node => {
            let mut n = model.nodes[&id].clone();
            n.name = name;
            // Rebinding keeps the world position; only the offset changes.
            n.level = v.entity("level", "Level", model, EntityKind::Level)?;
            for i in 0..3 {
                n.position[i] = Length::from_si(v.qty(
                    &format!("p{i}"),
                    Role::Length,
                    "Position",
                    n.position[i].si(),
                )?);
                n.spring_translation[i] = Stiffness::from_si(v.qty(
                    &format!("k{i}"),
                    Role::Stiffness,
                    "Spring",
                    n.spring_translation[i].si(),
                )?);
                n.mass[i] =
                    Mass::from_si(v.qty(&format!("m{i}"), Role::Mass, "Mass", n.mass[i].si())?);
            }
            if v.changed("offset") {
                if v.changed("p2") {
                    return Err("Edit either Z or the offset above the level, not both".into());
                }
                let offset = v.qty("offset", Role::Length, "Offset", 0.0)?;
                let elevation = model
                    .levels
                    .get(&n.level)
                    .map(|l| l.elevation.si())
                    .ok_or("Level: choose a level")?;
                n.position[2] = Length::from_si(elevation + offset);
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
            e.roll = Angle::from_si(v.qty("roll", Role::Angle, "Roll", e.roll.si())?);
            let md = &mut e.modifiers;
            for (key, name, value) in [
                ("mod_iy", "Iy modifier", &mut md.iy),
                ("mod_iz", "Iz modifier", &mut md.iz),
                ("mod_torsion", "J modifier", &mut md.torsion),
                ("mod_area", "A modifier", &mut md.area),
                ("mod_shear_y", "Shear y modifier", &mut md.shear_y),
                ("mod_shear_z", "Shear z modifier", &mut md.shear_z),
                ("mod_mass", "Mass modifier", &mut md.mass),
                ("mod_weight", "Weight modifier", &mut md.weight),
            ] {
                *value = v.num(key, name, *value)?;
            }
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
            e.thickness = Length::from_si(v.qty(
                "thickness",
                Role::Thickness,
                "Thickness",
                e.thickness.si(),
            )?);
            e.formulation = v.item("formulation", &FORMULATIONS, e.formulation);
            // A reference typed in elsewhere shows no choice and is kept.
            e.local_x = v.item("local_x", &SHELL_LOCAL_X, e.local_x);
            let md = &mut e.modifiers;
            for (key, name, value) in [
                ("mod_membrane_x", "Membrane x modifier", &mut md.membrane_x),
                ("mod_membrane_y", "Membrane y modifier", &mut md.membrane_y),
                (
                    "mod_membrane_shear",
                    "In-plane shear modifier",
                    &mut md.membrane_shear,
                ),
                ("mod_bending", "Bending modifier", &mut md.bending),
                ("mod_mass", "Mass modifier", &mut md.mass),
                ("mod_weight", "Weight modifier", &mut md.weight),
            ] {
                *value = v.num(key, name, *value)?;
            }
            (e != model.shells[&id]).then_some(Command::UpdateShell { id, shell: e })
        }
        EntityKind::Material => {
            let mut e = model.materials[&id].clone();
            e.name = name;
            e.young = Pressure::from_si(v.qty("young", Role::Stress, "E", e.young.si())?);
            e.poisson = v.num("poisson", "Poisson's ratio", e.poisson)?;
            e.density =
                MassDensity::from_si(v.qty("density", Role::Density, "Density", e.density.si())?);
            for (key, name, strength) in [
                ("fy", "Fy", &mut e.fy),
                ("fu", "Fu", &mut e.fu),
                ("fc", "f'c", &mut e.fc),
            ] {
                let current = strength.map(Pressure::si);
                *strength = v
                    .opt_qty(key, Role::Stress, name, current)?
                    .map(Pressure::from_si);
            }
            (e != model.materials[&id]).then_some(Command::UpdateMaterial { id, material: e })
        }
        EntityKind::Section => {
            let mut e = model.sections[&id].clone();
            e.name = name;
            e.area = Area::from_si(v.qty("area", Role::Area, "Area", e.area.si())?);
            e.iy = SecondMoment::from_si(v.qty("iy", Role::SecondMoment, "Iy", e.iy.si())?);
            e.iz = SecondMoment::from_si(v.qty("iz", Role::SecondMoment, "Iz", e.iz.si())?);
            e.torsion =
                SecondMoment::from_si(v.qty("torsion", Role::SecondMoment, "J", e.torsion.si())?);
            for (key, name, shear) in [
                ("shear_y", "Shear area y", &mut e.shear_y),
                ("shear_z", "Shear area z", &mut e.shear_z),
            ] {
                let current = shear.map(Area::si);
                *shear = v
                    .opt_qty(key, Role::Area, name, current)?
                    .map(Area::from_si);
            }
            (e != model.sections[&id]).then_some(Command::UpdateSection { id, section: e })
        }
        EntityKind::LoadCase => {
            let mut e = model.load_cases[&id].clone();
            e.name = name;
            for i in 0..3 {
                e.self_weight[i] = v.num(&format!("sw{i}"), "Self weight", e.self_weight[i])?;
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
                    v.num(&format!("term_factor_{i}"), "Factor", e.terms[i].1)?,
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
        EntityKind::MassSource => {
            let mut e = model.mass_sources[&id].clone();
            e.name = name;
            e.element_mass = v.check("element_mass");
            e.lateral = v.check("lateral");
            e.vertical = v.check("vertical");
            e.lump_to_levels = v.check("lump");
            (e != model.mass_sources[&id])
                .then_some(Command::UpdateMassSource { id, mass_source: e })
        }
        EntityKind::Underlay => {
            let mut e = model.underlays[&id].clone();
            e.name = name;
            e.level = v.entity("level", "Level", model, EntityKind::Level)?;
            for i in 0..2 {
                e.origin[i] = Length::from_si(v.qty(
                    &format!("o{i}"),
                    Role::Length,
                    "Origin",
                    e.origin[i].si(),
                )?);
            }
            (e != model.underlays[&id]).then_some(Command::UpdateUnderlay { id, underlay: e })
        }
    })
}

// MARK: Widgets

struct TextField {
    key: String,
    label: SharedString,
    input: Entity<InputState>,
    committed: String,
    span: u16,
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
    span: u16,
}

/// One field's widget, kept in the order the specs list them.
enum Widget {
    Text(TextField),
    Check(CheckField),
    Choice(ChoiceField),
}

struct Single {
    id: EntityId,
    kind: EntityKind,
    fields: Vec<Widget>,
    /// The plane of bending the Results tab plots, for a frame.
    plane: Plane,
    _subscriptions: Vec<Subscription>,
}

impl Single {
    fn texts(&self) -> impl Iterator<Item = &TextField> {
        self.fields.iter().filter_map(|f| match f {
            Widget::Text(t) => Some(t),
            _ => None,
        })
    }
}

struct Multi {
    section: Choice,
    material: Choice,
    /// The level to bind every selected node to, keeping their positions.
    level: Choice,
    /// Stiffness modifier presets for the selected frames and shells.
    frame_modifiers: Choice,
    shell_modifiers: Choice,
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
    /// Kept across selections, so browsing results from frame to frame stays on the tab.
    tab: EditorTab,
    /// Selection, revision, and display precision the widgets were built for.
    built_for: (Vec<EntityId>, u64, usize),
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
            tab: EditorTab::Properties,
            built_for: (vec![], u64::MAX, 0),
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
        let built_for = (selection.clone(), revision, crate::text::precision());
        if self.built_for == built_for {
            return;
        }
        self.built_for = built_for;
        self.shown = match selection.as_slice() {
            [] => Shown::Nothing,
            [id] => self
                .build_single(*id, window, cx)
                .map_or(Shown::Nothing, Shown::Single),
            _ => Shown::Multi(self.build_multi(window, cx)),
        };
        if let Some(key) = self.pending_focus.take()
            && let Shown::Single(single) = &self.shown
            && let Some(field) = single.texts().find(|t| t.key == key)
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
        let document = self.document.read(cx);
        let (kind, specs) = specs(document.model(), id)?;
        let plane = document
            .analysis()
            .and_then(|analysis| analysis.member_diagram(id))
            .map(Plane::dominant)
            .unwrap_or(Plane::XY);
        let mut single = Single {
            id,
            kind,
            fields: vec![],
            plane,
            _subscriptions: vec![],
        };
        for spec in specs {
            match spec {
                FieldSpec::Text {
                    key,
                    label,
                    value,
                    span,
                } => {
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
                    single.fields.push(Widget::Text(TextField {
                        key,
                        label,
                        input,
                        committed: value,
                        span,
                    }));
                }
                FieldSpec::Check {
                    key,
                    group,
                    label,
                    value,
                } => {
                    single.fields.push(Widget::Check(CheckField {
                        key,
                        group,
                        label,
                        value,
                    }));
                }
                FieldSpec::Choice {
                    key,
                    label,
                    options,
                    selected,
                    span,
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
                    single.fields.push(Widget::Choice(ChoiceField {
                        key,
                        label,
                        select,
                        span,
                    }));
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
        let levels: Vec<SharedString> = rows_of(model, EntityKind::Level)
            .into_iter()
            .map(|(_, n)| n.into())
            .collect();
        let section =
            cx.new(|cx| SelectState::new(SearchableVec::from(sections), None, window, cx));
        let material =
            cx.new(|cx| SelectState::new(SearchableVec::from(materials), None, window, cx));
        let level = cx.new(|cx| SelectState::new(SearchableVec::from(levels), None, window, cx));
        let frame_modifiers = cx.new(|cx| {
            SelectState::new(
                SearchableVec::from(labels(&FRAME_PRESETS)),
                None,
                window,
                cx,
            )
        });
        let shell_modifiers = cx.new(|cx| {
            SelectState::new(
                SearchableVec::from(labels(&SHELL_PRESETS)),
                None,
                window,
                cx,
            )
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &frame_modifiers,
                window,
                |this, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    let SelectEvent::Confirm(Some(name)) = event else {
                        return;
                    };
                    this.assign_frame_modifiers(name.clone(), window, cx);
                },
            ),
            cx.subscribe_in(
                &shell_modifiers,
                window,
                |this, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    let SelectEvent::Confirm(Some(name)) = event else {
                        return;
                    };
                    this.assign_shell_modifiers(name.clone(), window, cx);
                },
            ),
            cx.subscribe_in(
                &level,
                window,
                |this, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    let SelectEvent::Confirm(Some(name)) = event else {
                        return;
                    };
                    this.assign_level_to_nodes(name.clone(), window, cx);
                },
            ),
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
            level,
            frame_modifiers,
            shell_modifiers,
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
        for field in &single.fields {
            match field {
                Widget::Text(t) => {
                    values
                        .texts
                        .insert(t.key.clone(), t.input.read(cx).value().to_string());
                    values.committed.insert(t.key.clone(), t.committed.clone());
                }
                Widget::Check(c) => {
                    values.checks.insert(c.key.clone(), c.value);
                }
                Widget::Choice(c) => {
                    values.choices.insert(
                        c.key.clone(),
                        c.select.read(cx).selected_index(cx).map(|ix| ix.row),
                    );
                }
            }
        }
        let (id, kind) = (single.id, single.kind);
        let unchanged = single.texts().all(|t| values.text(&t.key) == t.committed);
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

    /// Rebuilds the fields after the display precision changed, so their text
    /// is written afresh.
    pub fn reformat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync(window, cx);
    }

    pub fn set_tab(&mut self, tab: EditorTab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn set_plane(&mut self, plane: Plane, cx: &mut Context<Self>) {
        if let Shown::Single(single) = &mut self.shown {
            single.plane = plane;
            cx.notify();
        }
    }

    fn set_check(&mut self, key: String, value: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Shown::Single(single) = &mut self.shown
            && let Some(field) = single.fields.iter_mut().find_map(|f| match f {
                Widget::Check(c) if c.key == key => Some(c),
                _ => None,
            })
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

    fn assign_frame_modifiers(
        &mut self,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = {
            let document = self.document.read(cx);
            let frames = document.selected_of(EntityKind::Frame);
            frame_modifier_commands(document.model(), &frames, &name)
        };
        if !commands.is_empty() {
            self.apply(Command::Batch { commands }, window, cx);
        }
    }

    fn assign_shell_modifiers(
        &mut self,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = {
            let document = self.document.read(cx);
            let shells = document.selected_of(EntityKind::Shell);
            shell_modifier_commands(document.model(), &shells, &name)
        };
        if !commands.is_empty() {
            self.apply(Command::Batch { commands }, window, cx);
        }
    }

    /// Binds every selected node to the named level. Positions stay; the
    /// nodes' offsets change.
    fn assign_level_to_nodes(
        &mut self,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = {
            let document = self.document.read(cx);
            let model = document.model();
            let Some(level) = model.find::<Level>(&name) else {
                return;
            };
            document
                .selected_of(EntityKind::Node)
                .into_iter()
                .filter(|id| model.nodes[id].level != level)
                .map(|id| {
                    let mut node = model.nodes[&id].clone();
                    node.level = level;
                    Command::UpdateNode { id, node }
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
        let mut form = form();
        let mut fields = single.fields.iter().peekable();
        while let Some(field) = fields.next() {
            form = form.child(match field {
                Widget::Text(t) => Field::new()
                    .col_span(t.span)
                    .label(t.label.clone())
                    .child(Input::new(&t.input).small()),
                Widget::Choice(c) => Field::new()
                    .col_span(c.span)
                    .label(c.label.clone())
                    .child(Select::new(&c.select).small()),
                Widget::Check(first) => {
                    let mut group = vec![first];
                    while let Some(Widget::Check(next)) = fields.peek()
                        && next.group == first.group
                    {
                        group.push(next);
                        fields.next();
                    }
                    let boxes = group.into_iter().map(|c| {
                        let key = c.key.clone();
                        Checkbox::new(SharedString::from(format!("check-{}", c.key)))
                            .small()
                            .label(c.label.clone())
                            .checked(c.value)
                            .on_change(cx.listener(move |this, value: &bool, window, cx| {
                                this.set_check(key.clone(), *value, window, cx)
                            }))
                    });
                    Field::new()
                        .col_span(FULL)
                        .label(first.group.clone())
                        .child(h_flex().flex_wrap().gap_x_3().gap_y_1().children(boxes))
                }
            });
        }
        let extras = self.render_extras(single, window, cx);
        v_flex()
            .gap_4()
            .child(form)
            .children(extras)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Button::new("delete-entity")
                            .small()
                            .danger()
                            .label("Delete")
                            .on_click(delete_and_close),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("id #{}", single.id.0)),
                    ),
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
            EntityKind::Level => {
                let on_level = oa_model::levels::membership(model, id);
                let height = oa_model::levels::height_below(model, id)
                    .map(|h| format!("{} {} above the level below", fmt_q(Role::Length, h), UNITS.symbol(Role::Length)))
                    .unwrap_or_else(|| "The lowest level".into());
                v_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(muted).child(height))
                    .child(div().text_xs().text_color(muted).child(format!(
                        "{} nodes, {} frames, {} shells on this level",
                        on_level.nodes.len(),
                        on_level.frames.len(),
                        on_level.shells.len()
                    )))
                    .child(
                        Button::new("go-to-level")
                            .small()
                            .outline()
                            .label("Make active")
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(Box::new(SetActiveLevel(id.0)), cx)
                            }),
                    )
                    .into_any_element()
            }
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
        let frames = document.selected_of(EntityKind::Frame).len();
        let shells = document.selected_of(EntityKind::Shell).len();
        let nodes = document.selected_of(EntityKind::Node).len();
        let restraints = multi.restraints;
        let mut form = form();
        if frames > 0 {
            form = form
                .child(
                    Field::new()
                        .col_span(HALF)
                        .label(format!("Assign section to {frames} frames"))
                        .child(
                            Select::new(&multi.section)
                                .small()
                                .placeholder("Choose a section"),
                        ),
                )
                .child(
                    Field::new()
                        .col_span(HALF)
                        .label(format!("Assign material to {frames} frames"))
                        .child(
                            Select::new(&multi.material)
                                .small()
                                .placeholder("Choose a material"),
                        ),
                )
                .child(
                    Field::new()
                        .col_span(FULL)
                        .label(format!("Stiffness modifiers of {frames} frames"))
                        .child(
                            Select::new(&multi.frame_modifiers)
                                .small()
                                .placeholder("Choose cracked-section stiffness"),
                        ),
                );
        }
        if shells > 0 {
            form = form.child(
                Field::new()
                    .col_span(FULL)
                    .label(format!("Stiffness modifiers of {shells} shells"))
                    .child(
                        Select::new(&multi.shell_modifiers)
                            .small()
                            .placeholder("Choose cracked-section stiffness"),
                    ),
            );
        }
        if nodes > 0 {
            let boxes = (0..6).map(|i| {
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
            });
            form = form
                .child(
                    Field::new()
                        .col_span(FULL)
                        .label(format!("Bind {nodes} nodes to level"))
                        .child(
                            Select::new(&multi.level)
                                .small()
                                .placeholder("Choose a level"),
                        ),
                )
                .child(
                    Field::new()
                        .col_span(FULL)
                        .label(format!("Set restraints of {nodes} nodes"))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .items_center()
                                .gap_x_3()
                                .gap_y_1()
                                .children(boxes)
                                .child(
                                    Button::new("apply-restraints")
                                        .small()
                                        .outline()
                                        .ml_auto()
                                        .label("Apply")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.assign_restraints(window, cx)
                                        })),
                                ),
                        ),
                );
        }
        v_flex()
            .gap_4()
            .when(frames > 0 || shells > 0 || nodes > 0, |this| {
                this.child(form)
            })
            .child(
                h_flex().child(
                    Button::new("delete-selected")
                        .small()
                        .danger()
                        .label("Delete selected")
                        .on_click(delete_and_close),
                ),
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

/// The editor's form: the six-column grid with small labels.
fn form() -> Form {
    Form::new()
        .small()
        .columns(COLUMNS)
        .label_text_size(rems(0.75))
}

/// Deletes the selection and closes the editor, which would otherwise be
/// left open on nothing.
fn delete_and_close(_: &ClickEvent, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(DeleteSelected), cx);
    window.close_dialog(cx);
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

impl PropertyEditor {
    /// What the editor shows, for its dialog's title: "Frame F7", or the
    /// selection's make-up, "18 nodes, 26 frames".
    pub fn title(&self, cx: &App) -> SharedString {
        let document = self.document.read(cx);
        match &self.shown {
            Shown::Nothing => "Properties".into(),
            Shown::Single(single) => {
                let name = document.model().name_of(single.id).unwrap_or("?");
                format!("{} {name}", crate::text::capitalize(&single.kind.to_string())).into()
            }
            Shown::Multi(_) => selection_summary(document).into(),
        }
    }
}

impl Render for PropertyEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        // A frame with results gets a Results tab: its diagram for the shown combination.
        let results = match &self.shown {
            Shown::Single(single) if single.kind == EntityKind::Frame => {
                self.document.read(cx).analysis().and_then(|analysis| {
                    let diagram = analysis.member_diagram(single.id)?.clone();
                    let combination = analysis
                        .results
                        .combinations
                        .get(analysis.combination)?
                        .combination
                        .clone();
                    Some((diagram, combination, single.plane))
                })
            }
            _ => None,
        };
        let tabs = results.is_some().then(|| {
            div().child(
                TabBar::new("property-tabs")
                    .underline()
                    .small()
                    .selected_index(match self.tab {
                        EditorTab::Properties => 0,
                        EditorTab::Results => 1,
                    })
                    .child(Tab::new().label("Properties"))
                    .child(Tab::new().label("Results"))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        let tab = if *ix == 1 {
                            EditorTab::Results
                        } else {
                            EditorTab::Properties
                        };
                        this.set_tab(tab, cx)
                    })),
            )
        });
        let body = match (&self.shown, results) {
            (Shown::Single(_), Some((diagram, combination, plane)))
                if self.tab == EditorTab::Results =>
            {
                render_member_results(
                    &diagram,
                    &combination,
                    plane,
                    cx.listener(|this, plane: &Plane, _, cx| this.set_plane(*plane, cx)),
                    window,
                    cx,
                )
            }
            (Shown::Nothing, _) => div()
                .text_sm()
                .text_color(muted)
                .child("Select an entity in the view or the model tree to edit it.")
                .into_any_element(),
            (Shown::Single(single), _) => self.render_single(single, window, cx),
            (Shown::Multi(multi), _) => self.render_multi(multi, cx),
        };
        // The dialog around the editor gives it its title and scrolls it.
        v_flex().gap_3().pb_1().children(tabs).child(body)
    }
}


#[cfg(test)]
mod tests {
    use super::{
        FULL, FieldSpec, Values, command_for, frame_modifier_commands, shell_modifier_commands,
        specs,
    };
    use crate::text::{DEFAULT_PRECISION, TestPrecision};
    use oa_core::units::Length;
    use oa_model::{
        Command, ElevationScope, EntityId, EntityKind, Frame, FrameModifiers, Model, Node, Shell,
        ShellFormulation, ShellModifiers,
    };

    /// The panel as just built: every text field shows its committed text
    /// and every choice its current selection.
    fn values_for(model: &Model, id: oa_model::EntityId) -> Values {
        let (_, fields) = specs(model, id).unwrap();
        let mut v = Values::default();
        for f in fields {
            match f {
                FieldSpec::Text { key, value, .. } => {
                    v.texts.insert(key.clone(), value.clone());
                    v.committed.insert(key, value);
                }
                FieldSpec::Choice { key, selected, .. } => {
                    v.choices.insert(key, selected);
                }
                FieldSpec::Check { .. } => {}
            }
        }
        v
    }

    /// The editor lays fields out in this order, so a node's level sits with
    /// its position rather than after its springs and masses, and a group of
    /// checkboxes starts on a fresh row.
    #[test]
    fn node_fields_read_in_order_on_whole_rows() {
        let _p = TestPrecision::of(DEFAULT_PRECISION);
        let mut model = Model::default();
        let level = model.base_level().unwrap();
        let id = model.insert(Node::new("N1", level, [Length::ZERO; 3]));
        let (_, fields) = specs(&model, id).unwrap();
        let keys: Vec<&str> = fields
            .iter()
            .map(|f| match f {
                FieldSpec::Text { key, .. }
                | FieldSpec::Check { key, .. }
                | FieldSpec::Choice { key, .. } => key.as_str(),
            })
            .collect();
        assert_eq!(keys[..6], ["name", "p0", "p1", "p2", "level", "offset"]);
        let mut filled = 0;
        for f in &fields {
            match f {
                FieldSpec::Text { span, .. } | FieldSpec::Choice { span, .. } => filled += span,
                FieldSpec::Check { .. } => assert_eq!(filled % FULL, 0, "a row left part-filled"),
            }
        }
        assert_eq!(filled % FULL, 0);
    }

    #[test]
    fn untouched_fields_keep_their_stored_value() {
        let _p = TestPrecision::of(DEFAULT_PRECISION);
        // 6 m shows as 19.69 ft, which is not 6 m when parsed back.
        let mut model = Model::default();
        let level = model.base_level().unwrap();
        let id = model.insert(Node::new("N1", level, [Length::from_metres(6.0); 3]));
        let v = values_for(&model, id);
        assert_eq!(command_for(&model, id, EntityKind::Node, &v).unwrap(), None);

        let mut v = values_for(&model, id);
        v.texts.insert("p0".into(), "20".into());
        let Some(Command::UpdateNode { node, .. }) =
            command_for(&model, id, EntityKind::Node, &v).unwrap()
        else {
            panic!("an edited coordinate is a command");
        };
        assert!((node.position[0].si() - 6.096).abs() < 1e-12);
        assert_eq!(node.position[1].si(), 6.0);
    }

    #[test]
    fn offset_and_level_edits_write_the_position() {
        let _p = TestPrecision::of(DEFAULT_PRECISION);
        let mut model = Model::default();
        let base = model.base_level().unwrap();
        let upper = model.insert(oa_model::Level::new("L1", Length::from_feet(12.0)));
        let id = model.insert(Node::new("N1", base, [Length::ZERO; 3]));
        // An offset of 2 ft above Base puts the node at 2 ft.
        let mut v = values_for(&model, id);
        v.texts.insert("offset".into(), "2".into());
        let Some(Command::UpdateNode { node, .. }) =
            command_for(&model, id, EntityKind::Node, &v).unwrap()
        else {
            panic!("an edited offset is a command");
        };
        assert!((node.position[2].si() - Length::from_feet(2.0).si()).abs() < 1e-12);
        // Rebinding to L1 keeps the node where it is; its offset becomes -12 ft.
        let mut v = values_for(&model, id);
        let rows = crate::explorer::rows_of(&model, EntityKind::Level);
        v.choices.insert(
            "level".into(),
            rows.iter().position(|(l, _)| *l == upper),
        );
        let Some(Command::UpdateNode { node, .. }) =
            command_for(&model, id, EntityKind::Node, &v).unwrap()
        else {
            panic!("a rebinding is a command");
        };
        assert_eq!(node.level, upper);
        assert_eq!(node.position[2].si(), 0.0);
        // Z and offset edited together is ambiguous and refused.
        let mut v = values_for(&model, id);
        v.texts.insert("offset".into(), "1".into());
        v.texts.insert("p2".into(), "1".into());
        assert!(command_for(&model, id, EntityKind::Node, &v).is_err());
        // A level's elevation edit becomes a move of that level alone.
        let mut v = values_for(&model, upper);
        v.texts.insert("elevation".into(), "14".into());
        let Some(Command::SetLevelElevation { scope, elevation, .. }) =
            command_for(&model, upper, EntityKind::Level, &v).unwrap()
        else {
            panic!("an elevation edit moves the level");
        };
        assert_eq!(scope, ElevationScope::ThisLevel);
        assert!((elevation.si() - Length::from_feet(14.0).si()).abs() < 1e-12);
    }

    #[test]
    fn material_strengths_show_blank_when_absent_and_clear_when_blanked() {
        let _p = TestPrecision::of(DEFAULT_PRECISION);
        let mut model = Model::default();
        let steel = oa_model::Library::starter()
            .material("A992", "steel")
            .unwrap();
        let id = model.insert(steel.clone());
        let v = values_for(&model, id);
        assert_eq!(v.texts["fy"], "50.00");
        assert_eq!(v.texts["fc"], "");
        assert_eq!(
            command_for(&model, id, EntityKind::Material, &v).unwrap(),
            None
        );

        // Blanking Fu removes it; typing f'c sets it in ksi.
        let mut v = values_for(&model, id);
        v.texts.insert("fu".into(), " ".into());
        v.texts.insert("fc".into(), "4".into());
        let Some(Command::UpdateMaterial { material, .. }) =
            command_for(&model, id, EntityKind::Material, &v).unwrap()
        else {
            panic!("an edited strength is a command");
        };
        assert_eq!(material.fy, steel.fy);
        assert_eq!(material.fu, None);
        assert!((material.fc.unwrap().si() - 4.0 * 6.894_757_293_168e6).abs() < 1e-3);
        let mut v = values_for(&model, id);
        v.texts.insert("fy".into(), "fifty".into());
        assert!(command_for(&model, id, EntityKind::Material, &v).is_err());
    }

    #[test]
    fn shear_areas_show_in_square_inches_and_clear_when_blanked() {
        let _p = TestPrecision::of(DEFAULT_PRECISION);
        let mut model = Model::default();
        let beam = oa_model::Library::aisc()
            .section("W14X90", "beam")
            .unwrap();
        let id = model.insert(beam.clone());
        let v = values_for(&model, id);
        assert_eq!(v.texts["shear_y"], "6.16");
        assert_eq!(v.texts["shear_z"], "17.16");
        assert_eq!(
            command_for(&model, id, EntityKind::Section, &v).unwrap(),
            None
        );

        // Blanking one makes that plane rigid in shear; the other stays.
        let mut v = values_for(&model, id);
        v.texts.insert("shear_y".into(), "".into());
        let Some(Command::UpdateSection { section, .. }) =
            command_for(&model, id, EntityKind::Section, &v).unwrap()
        else {
            panic!("an edited shear area is a command");
        };
        assert_eq!(section.shear_y, None);
        assert_eq!(section.shear_z, beam.shear_z);
        let mut v = values_for(&model, id);
        v.texts.insert("shear_z".into(), "web".into());
        assert!(command_for(&model, id, EntityKind::Section, &v).is_err());
    }

    /// A frame and a shell on four nodes, with a material and a section.
    fn frame_and_shell() -> (Model, EntityId, EntityId) {
        let mut model = Model::default();
        let level = model.base_level().unwrap();
        let library = oa_model::Library::starter();
        let material = model.insert(library.material("Concrete 4 ksi", "concrete").unwrap());
        let section = model.insert(library.section("W14x90", "beam").unwrap());
        let nodes = [(0.0, 0.0), (6.0, 0.0), (6.0, 4.0), (0.0, 4.0)].map(|(x, z)| {
            let p = [Length::from_si(x), Length::ZERO, Length::from_si(z)];
            model.insert(Node::new("N", level, p))
        });
        let frame = model.insert(Frame::new("B1", [nodes[0], nodes[1]], material, section));
        let shell = model.insert(Shell {
            name: "W1".into(),
            nodes,
            material,
            thickness: Length::from_inches(8.0),
            formulation: ShellFormulation::Dkmq,
            drilling_ratio: 1e-3,
            local_x: None,
            modifiers: ShellModifiers::default(),
        });
        (model, frame, shell)
    }

    #[test]
    fn stiffness_modifiers_edit_as_plain_numbers_on_whole_rows() {
        let _p = TestPrecision::of(DEFAULT_PRECISION);
        let (model, frame, shell) = frame_and_shell();
        for id in [frame, shell] {
            let (_, fields) = specs(&model, id).unwrap();
            let mut filled = 0;
            for f in &fields {
                match f {
                    FieldSpec::Text { span, .. } | FieldSpec::Choice { span, .. } => filled += span,
                    FieldSpec::Check { .. } => {
                        assert_eq!(filled % FULL, 0, "a row left part-filled")
                    }
                }
            }
            assert_eq!(filled % FULL, 0);
        }
        let v = values_for(&model, frame);
        assert_eq!(v.texts["mod_iz"], "1.00");
        assert_eq!(
            command_for(&model, frame, EntityKind::Frame, &v).unwrap(),
            None
        );

        let mut v = values_for(&model, frame);
        v.texts.insert("mod_iz".into(), "0.35".into());
        let Some(Command::UpdateFrame { frame: edited, .. }) =
            command_for(&model, frame, EntityKind::Frame, &v).unwrap()
        else {
            panic!("an edited modifier is a command");
        };
        assert_eq!(
            edited.modifiers,
            FrameModifiers {
                iz: 0.35,
                ..Default::default()
            }
        );
        // Membrane y along a local x that runs up the wall.
        let mut v = values_for(&model, shell);
        assert_eq!(v.choices["local_x"], Some(0));
        v.texts.insert("mod_membrane_y".into(), "0.7".into());
        v.choices.insert("local_x".into(), Some(3));
        let Some(Command::UpdateShell { shell: edited, .. }) =
            command_for(&model, shell, EntityKind::Shell, &v).unwrap()
        else {
            panic!("an edited modifier is a command");
        };
        assert_eq!(edited.modifiers.membrane_y, 0.7);
        assert_eq!(edited.modifiers.membrane_x, 1.0);
        assert_eq!(edited.local_x, Some([0.0, 0.0, 1.0]));
        // A reference the choices do not list is kept rather than reset.
        let mut custom = model.clone();
        custom.shells.get_mut(&shell).unwrap().local_x = Some([1.0, 0.0, 1.0]);
        let v = values_for(&custom, shell);
        assert_eq!(v.choices["local_x"], None);
        assert_eq!(
            command_for(&custom, shell, EntityKind::Shell, &v).unwrap(),
            None
        );
        let mut v = values_for(&model, shell);
        v.texts.insert("mod_bending".into(), "cracked".into());
        assert!(command_for(&model, shell, EntityKind::Shell, &v).is_err());
    }

    #[test]
    fn presets_assign_aci_cracked_sections_to_the_selection() {
        let (mut model, frame, shell) = frame_and_shell();
        let commands = frame_modifier_commands(&model, &[frame], "ACI 318 beam: 0.35 Ig");
        let [Command::UpdateFrame { frame: beam, .. }] = &commands[..] else {
            panic!("one update per frame");
        };
        assert_eq!((beam.modifiers.iy, beam.modifiers.iz), (0.35, 0.35));
        assert_eq!(beam.modifiers.area, 1.0);
        // A frame that already has the preset is left alone.
        model.frames.insert(frame, beam.clone());
        assert!(frame_modifier_commands(&model, &[frame], "ACI 318 beam: 0.35 Ig").is_empty());
        assert!(frame_modifier_commands(&model, &[frame], "no such preset").is_empty());

        let commands = shell_modifier_commands(&model, &[shell], "ACI 318 flat slab: 0.25 Ig");
        let [Command::UpdateShell { shell: slab, .. }] = &commands[..] else {
            panic!("one update per shell");
        };
        assert_eq!(
            slab.modifiers,
            ShellModifiers {
                bending: 0.25,
                ..Default::default()
            }
        );
        let commands = shell_modifier_commands(&model, &[shell], "ACI 318 wall, cracked: 0.35 Ig");
        let [Command::UpdateShell { shell: wall, .. }] = &commands[..] else {
            panic!("one update per shell");
        };
        assert_eq!(
            (wall.modifiers.membrane_x, wall.modifiers.membrane_y),
            (0.35, 0.35)
        );
        assert_eq!(wall.modifiers.membrane_shear, 1.0);
    }
}
