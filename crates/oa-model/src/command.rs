//! Every edit is a command that returns its inverse. Undo and redo are a
//! stack of commands and nothing else. A GUI, an agent, and a file import
//! all issue the same commands.
use crate::levels::{ElevationScope, TOLERANCE};
use crate::{entity::*, model::*};
use oa_core::units::{Acceleration, Length};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum ModelError {
    #[error("no entity #{}", .0.0)]
    NotFound(EntityId),
    #[error("id #{} is already in use", .0.0)]
    IdInUse(EntityId),
    #[error("{kind} name {name:?} is already used")]
    DuplicateName { kind: EntityKind, name: String },
    #[error("{entity} references missing {kind} #{}", target.0)]
    Dangling {
        entity: String,
        target: EntityId,
        kind: EntityKind,
    },
    #[error("{entity} is still referenced by {by:?}; remove those first")]
    Referenced { entity: String, by: Vec<String> },
    #[error("{entity} is a {actual}, not a {expected}")]
    WrongKind {
        entity: String,
        expected: EntityKind,
        actual: EntityKind,
    },
    #[error("batch command {index} failed: {source}")]
    Batch {
        index: usize,
        source: Box<ModelError>,
    },
    #[error("{0}")]
    Invalid(String),
}
pub type Result<T> = std::result::Result<T, ModelError>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    AddLevel {
        id: EntityId,
        level: Level,
    },
    /// Renames a level or re-datums it in place: its nodes keep their world
    /// coordinates, so their offsets change. To move a floor use
    /// `SetLevelElevation`.
    UpdateLevel {
        id: EntityId,
        level: Level,
    },
    /// Refused while any node or underlay binds to the level, and for the
    /// last level.
    RemoveLevel {
        id: EntityId,
    },
    /// Moves a datum with the nodes bound to it, and with the levels above
    /// when the scope says so, keeping every node's offset. Applied as a
    /// batch of level and node updates, so undo restores the stored values.
    SetLevelElevation {
        id: EntityId,
        elevation: Length,
        scope: ElevationScope,
    },
    AddNode {
        id: EntityId,
        node: Node,
    },
    UpdateNode {
        id: EntityId,
        node: Node,
    },
    RemoveNode {
        id: EntityId,
    },
    AddMaterial {
        id: EntityId,
        material: Material,
    },
    UpdateMaterial {
        id: EntityId,
        material: Material,
    },
    RemoveMaterial {
        id: EntityId,
    },
    AddSection {
        id: EntityId,
        section: Section,
    },
    UpdateSection {
        id: EntityId,
        section: Section,
    },
    RemoveSection {
        id: EntityId,
    },
    AddFrame {
        id: EntityId,
        frame: Frame,
    },
    UpdateFrame {
        id: EntityId,
        frame: Frame,
    },
    RemoveFrame {
        id: EntityId,
    },
    AddShell {
        id: EntityId,
        shell: Shell,
    },
    UpdateShell {
        id: EntityId,
        shell: Shell,
    },
    RemoveShell {
        id: EntityId,
    },
    AddDiaphragm {
        id: EntityId,
        diaphragm: Diaphragm,
    },
    UpdateDiaphragm {
        id: EntityId,
        diaphragm: Diaphragm,
    },
    RemoveDiaphragm {
        id: EntityId,
    },
    AddLoadCase {
        id: EntityId,
        load_case: LoadCase,
    },
    UpdateLoadCase {
        id: EntityId,
        load_case: LoadCase,
    },
    RemoveLoadCase {
        id: EntityId,
    },
    AddCombination {
        id: EntityId,
        combination: Combination,
    },
    UpdateCombination {
        id: EntityId,
        combination: Combination,
    },
    RemoveCombination {
        id: EntityId,
    },
    AddGroup {
        id: EntityId,
        group: Group,
    },
    UpdateGroup {
        id: EntityId,
        group: Group,
    },
    RemoveGroup {
        id: EntityId,
    },
    AddUnderlay {
        id: EntityId,
        underlay: Underlay,
    },
    UpdateUnderlay {
        id: EntityId,
        underlay: Underlay,
    },
    RemoveUnderlay {
        id: EntityId,
    },
    /// A grid line needs a label and two distinct, finite ends in plan.
    AddGridLine {
        id: EntityId,
        grid_line: GridLine,
    },
    UpdateGridLine {
        id: EntityId,
        grid_line: GridLine,
    },
    RemoveGridLine {
        id: EntityId,
    },
    SetGravity {
        gravity: Acceleration,
    },
    SetMetadata {
        metadata: Metadata,
    },
    /// A mass source's cases must be load cases, each listed once with a
    /// positive multiplier, and it must keep lateral or vertical mass; it
    /// lumps only lateral mass. A listed case that carries self-weight while
    /// element mass is on would count the members' mass twice, which
    /// compilation reports.
    AddMassSource {
        id: EntityId,
        mass_source: MassSource,
    },
    UpdateMassSource {
        id: EntityId,
        mass_source: MassSource,
    },
    /// Refused while the source is the model's default.
    RemoveMassSource {
        id: EntityId,
    },
    /// The source modal and spectrum analysis use unless told otherwise.
    /// None means element and node mass in every direction.
    SetDefaultMassSource {
        id: Option<EntityId>,
    },
    /// Applied in order; rolled back completely if any command fails.
    Batch {
        commands: Vec<Command>,
    },
}

fn check_entity<T: Entity>(model: &Model, id: EntityId, e: &T) -> Result<()> {
    if e.name().trim().is_empty() {
        return Err(ModelError::Invalid(format!("{} needs a name", T::KIND)));
    }
    if T::table(model)
        .iter()
        .any(|(other, x)| *other != id && x.name() == e.name())
    {
        return Err(ModelError::DuplicateName {
            kind: T::KIND,
            name: e.name().into(),
        });
    }
    for (target, kind) in e.references() {
        match model.kind_of(target) {
            Some(k) if k == kind => {}
            Some(actual) => {
                return Err(ModelError::WrongKind {
                    entity: model.describe(target),
                    expected: kind,
                    actual,
                });
            }
            None => {
                return Err(ModelError::Dangling {
                    entity: format!("{} {:?}", T::KIND, e.name()),
                    target,
                    kind,
                });
            }
        }
    }
    Ok(())
}
fn add<T: Entity>(model: &mut Model, id: EntityId, e: T) -> Result<()> {
    if model.kind_of(id).is_some() {
        return Err(ModelError::IdInUse(id));
    }
    check_entity(model, id, &e)?;
    if T::KIND == EntityKind::Group {
        check_group_members(model, &e)?;
    }
    model.reserve(id);
    T::table_mut(model).insert(id, e);
    Ok(())
}
fn update<T: Entity>(model: &mut Model, id: EntityId, e: T) -> Result<T> {
    if !T::table(model).contains_key(&id) {
        return Err(match model.kind_of(id) {
            Some(actual) => ModelError::WrongKind {
                entity: model.describe(id),
                expected: T::KIND,
                actual,
            },
            None => ModelError::NotFound(id),
        });
    }
    check_entity(model, id, &e)?;
    if T::KIND == EntityKind::Group {
        check_group_members(model, &e)?;
    }
    Ok(T::table_mut(model).insert(id, e).unwrap())
}
/// Removes an entity and returns it with the prior state of every group that
/// held it, so the inverse can restore membership.
fn remove<T: Entity>(model: &mut Model, id: EntityId) -> Result<(T, Vec<(EntityId, Group)>)> {
    if !T::table(model).contains_key(&id) {
        return Err(match model.kind_of(id) {
            Some(actual) => ModelError::WrongKind {
                entity: model.describe(id),
                expected: T::KIND,
                actual,
            },
            None => ModelError::NotFound(id),
        });
    }
    let by = model.referrers(id);
    if !by.is_empty() {
        return Err(ModelError::Referenced {
            entity: model.describe(id),
            by: by.iter().map(|b| model.describe(*b)).collect(),
        });
    }
    let mut groups = vec![];
    for (gid, g) in model.groups.iter_mut() {
        if g.members.contains(&id) {
            groups.push((*gid, g.clone()));
            g.members.remove(&id);
        }
    }
    Ok((T::table_mut(model).remove(&id).unwrap(), groups))
}
/// Inverse of a removal: re-add, then restore any group memberships.
fn removal_inverse(add: Command, groups: Vec<(EntityId, Group)>) -> Command {
    if groups.is_empty() {
        return add;
    }
    let mut commands = vec![add];
    commands.extend(
        groups
            .into_iter()
            .map(|(id, group)| Command::UpdateGroup { id, group }),
    );
    Command::Batch { commands }
}
/// A level needs a finite elevation that no other level already sits at.
fn check_level(model: &Model, id: EntityId, level: &Level) -> Result<()> {
    if !level.elevation.si().is_finite() {
        return Err(ModelError::Invalid("level elevation must be finite".into()));
    }
    if let Some((other, _)) = model.levels.iter().find(|(other, l)| {
        **other != id && (l.elevation.si() - level.elevation.si()).abs() <= TOLERANCE
    }) {
        return Err(ModelError::Invalid(format!(
            "{} already sits at that elevation",
            model.describe(*other)
        )));
    }
    Ok(())
}
/// A strength a material gives must be positive and finite, and its
/// tensile strength no less than its yield stress.
fn check_material(material: &Material) -> Result<()> {
    for (name, strength) in [
        ("Fy", material.fy),
        ("Fu", material.fu),
        ("f'c", material.fc),
    ] {
        if strength.is_some_and(|s| !(s.si().is_finite() && s.si() > 0.0)) {
            return Err(ModelError::Invalid(format!(
                "material {name} must be positive and finite"
            )));
        }
    }
    if matches!((material.fy, material.fu), (Some(fy), Some(fu)) if fu.si() < fy.si()) {
        return Err(ModelError::Invalid(
            "material Fu must be at least Fy".into(),
        ));
    }
    Ok(())
}
/// A shear area a section gives must be positive and finite; leaving it out
/// is how a section is made rigid in shear.
fn check_section(section: &Section) -> Result<()> {
    if [section.shear_y, section.shear_z]
        .into_iter()
        .flatten()
        .any(|a| !(a.si().is_finite() && a.si() > 0.0))
    {
        return Err(ModelError::Invalid(
            "section shear areas must be positive and finite".into(),
        ));
    }
    Ok(())
}
/// A stiffness modifier multiplies a stiffness, so it must be positive and
/// finite; 1 leaves the stiffness unchanged.
fn check_modifiers(modifiers: &[f64]) -> Result<()> {
    if modifiers.iter().any(|f| !(f.is_finite() && *f > 0.0)) {
        return Err(ModelError::Invalid(
            "stiffness modifiers must be positive and finite".into(),
        ));
    }
    Ok(())
}
/// Mass and weight modifiers scale the member's own mass and self-weight;
/// zero leaves them out, as for a member another one already carries.
fn check_mass_weight(mass: f64, weight: f64) -> Result<()> {
    if [mass, weight].iter().any(|f| !(f.is_finite() && *f >= 0.0)) {
        return Err(ModelError::Invalid(
            "mass and weight modifiers must be finite and >= 0".into(),
        ));
    }
    Ok(())
}
/// An underlay is drawn as it is stored, so every coordinate must be finite.
fn check_underlay(underlay: &Underlay) -> Result<()> {
    let finite = |p: &[Length; 2]| p.iter().all(|v| v.si().is_finite());
    if !finite(&underlay.origin) || !underlay.segments.iter().flatten().all(finite) {
        return Err(ModelError::Invalid(
            "underlay coordinates must be finite".into(),
        ));
    }
    Ok(())
}
/// A grid line is drawn and snapped to as it is stored, so its ends must be
/// finite and apart.
fn check_grid_line(line: &GridLine) -> Result<()> {
    let [a, b] = [line.start, line.end].map(|p| p.map(|v| v.si()));
    if !a.iter().chain(&b).all(|v| v.is_finite()) {
        return Err(ModelError::Invalid(
            "grid line coordinates must be finite".into(),
        ));
    }
    if (a[0] - b[0]).hypot(a[1] - b[1]) <= TOLERANCE {
        return Err(ModelError::Invalid(format!(
            "grid line {:?} needs two different ends",
            line.name
        )));
    }
    Ok(())
}
/// Checks only what belongs to the source itself, plus references, which a
/// source keeps alive. Whether a listed case carries self-weight while
/// element mass is on depends on the case, which may change after the
/// source does, so compilation reports that instead: a check here would
/// refuse the inverse that undo or a batch rollback needs to restore a
/// source accepted earlier.
fn check_mass_source(model: &Model, source: &MassSource) -> Result<()> {
    if !source.lateral && !source.vertical {
        return Err(ModelError::Invalid(
            "a mass source needs lateral or vertical mass".into(),
        ));
    }
    for (i, &(case, multiplier)) in source.cases.iter().enumerate() {
        if !model.load_cases.contains_key(&case) {
            return Err(match model.kind_of(case) {
                Some(actual) => ModelError::WrongKind {
                    entity: model.describe(case),
                    expected: EntityKind::LoadCase,
                    actual,
                },
                None => ModelError::Dangling {
                    entity: "mass source".into(),
                    target: case,
                    kind: EntityKind::LoadCase,
                },
            });
        }
        if source.cases[..i].iter().any(|(c, _)| *c == case) {
            return Err(ModelError::Invalid(format!(
                "mass source lists {} twice",
                model.describe(case)
            )));
        }
        if !(multiplier.is_finite() && multiplier > 0.0) {
            return Err(ModelError::Invalid(
                "mass source multipliers must be positive and finite".into(),
            ));
        }
    }
    Ok(())
}
fn check_group_members<T: Entity>(model: &Model, e: &T) -> Result<()> {
    let json = serde_json::to_value(e).map_err(|x| ModelError::Invalid(x.to_string()))?;
    if let Some(members) = json.get("members").and_then(|m| m.as_array()) {
        for m in members {
            let id = EntityId(m.as_u64().unwrap_or(u64::MAX));
            if model.kind_of(id).is_none() {
                return Err(ModelError::Dangling {
                    entity: format!("group {:?}", e.name()),
                    target: id,
                    kind: EntityKind::Node,
                });
            }
        }
    }
    Ok(())
}

impl Command {
    /// Applies the command and returns the command that undoes it.
    pub fn apply(self, model: &mut Model) -> Result<Command> {
        use Command::*;
        Ok(match self {
            AddLevel { id, level } => {
                check_level(model, id, &level)?;
                add(model, id, level)?;
                RemoveLevel { id }
            }
            UpdateLevel { id, level } => {
                check_level(model, id, &level)?;
                UpdateLevel {
                    id,
                    level: update(model, id, level)?,
                }
            }
            RemoveLevel { id } => {
                if model.levels.contains_key(&id) && model.levels.len() == 1 {
                    return Err(ModelError::Invalid(
                        "a model keeps at least one level".into(),
                    ));
                }
                let (entity, groups) = remove(model, id)?;
                removal_inverse(AddLevel { id, level: entity }, groups)
            }
            SetLevelElevation {
                id,
                elevation,
                scope,
            } => {
                let plan = crate::levels::plan_set_elevation(model, id, elevation, scope)?;
                Batch {
                    commands: plan.commands,
                }
                .apply(model)?
            }
            AddNode { id, node } => {
                add(model, id, node)?;
                RemoveNode { id }
            }
            UpdateNode { id, node } => UpdateNode {
                id,
                node: update(model, id, node)?,
            },
            RemoveNode { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(AddNode { id, node: entity }, groups)
            }
            AddMaterial { id, material } => {
                check_material(&material)?;
                add(model, id, material)?;
                RemoveMaterial { id }
            }
            UpdateMaterial { id, material } => {
                check_material(&material)?;
                UpdateMaterial {
                    id,
                    material: update(model, id, material)?,
                }
            }
            RemoveMaterial { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(
                    AddMaterial {
                        id,
                        material: entity,
                    },
                    groups,
                )
            }
            AddSection { id, section } => {
                check_section(&section)?;
                add(model, id, section)?;
                RemoveSection { id }
            }
            UpdateSection { id, section } => {
                check_section(&section)?;
                UpdateSection {
                    id,
                    section: update(model, id, section)?,
                }
            }
            RemoveSection { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(
                    AddSection {
                        id,
                        section: entity,
                    },
                    groups,
                )
            }
            AddFrame { id, frame } => {
                check_modifiers(&frame.modifiers.values())?;
                frame.offsets.check().map_err(ModelError::Invalid)?;
                check_mass_weight(frame.modifiers.mass, frame.modifiers.weight)?;
                add(model, id, frame)?;
                RemoveFrame { id }
            }
            UpdateFrame { id, frame } => {
                check_modifiers(&frame.modifiers.values())?;
                frame.offsets.check().map_err(ModelError::Invalid)?;
                check_mass_weight(frame.modifiers.mass, frame.modifiers.weight)?;
                UpdateFrame {
                    id,
                    frame: update(model, id, frame)?,
                }
            }
            RemoveFrame { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(AddFrame { id, frame: entity }, groups)
            }
            AddShell { id, shell } => {
                check_modifiers(&shell.modifiers.values())?;
                check_mass_weight(shell.modifiers.mass, shell.modifiers.weight)?;
                add(model, id, shell)?;
                RemoveShell { id }
            }
            UpdateShell { id, shell } => {
                check_modifiers(&shell.modifiers.values())?;
                check_mass_weight(shell.modifiers.mass, shell.modifiers.weight)?;
                UpdateShell {
                    id,
                    shell: update(model, id, shell)?,
                }
            }
            RemoveShell { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(AddShell { id, shell: entity }, groups)
            }
            AddDiaphragm { id, diaphragm } => {
                add(model, id, diaphragm)?;
                RemoveDiaphragm { id }
            }
            UpdateDiaphragm { id, diaphragm } => UpdateDiaphragm {
                id,
                diaphragm: update(model, id, diaphragm)?,
            },
            RemoveDiaphragm { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(
                    AddDiaphragm {
                        id,
                        diaphragm: entity,
                    },
                    groups,
                )
            }
            AddLoadCase { id, load_case } => {
                add(model, id, load_case)?;
                RemoveLoadCase { id }
            }
            UpdateLoadCase { id, load_case } => UpdateLoadCase {
                id,
                load_case: update(model, id, load_case)?,
            },
            RemoveLoadCase { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(
                    AddLoadCase {
                        id,
                        load_case: entity,
                    },
                    groups,
                )
            }
            AddCombination { id, combination } => {
                add(model, id, combination)?;
                RemoveCombination { id }
            }
            UpdateCombination { id, combination } => UpdateCombination {
                id,
                combination: update(model, id, combination)?,
            },
            RemoveCombination { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(
                    AddCombination {
                        id,
                        combination: entity,
                    },
                    groups,
                )
            }
            AddGroup { id, group } => {
                add(model, id, group)?;
                RemoveGroup { id }
            }
            UpdateGroup { id, group } => UpdateGroup {
                id,
                group: update(model, id, group)?,
            },
            RemoveGroup { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(AddGroup { id, group: entity }, groups)
            }
            AddUnderlay { id, underlay } => {
                check_underlay(&underlay)?;
                add(model, id, underlay)?;
                RemoveUnderlay { id }
            }
            UpdateUnderlay { id, underlay } => {
                check_underlay(&underlay)?;
                UpdateUnderlay {
                    id,
                    underlay: update(model, id, underlay)?,
                }
            }
            RemoveUnderlay { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(
                    AddUnderlay {
                        id,
                        underlay: entity,
                    },
                    groups,
                )
            }
            AddGridLine { id, grid_line } => {
                check_grid_line(&grid_line)?;
                add(model, id, grid_line)?;
                RemoveGridLine { id }
            }
            UpdateGridLine { id, grid_line } => {
                check_grid_line(&grid_line)?;
                UpdateGridLine {
                    id,
                    grid_line: update(model, id, grid_line)?,
                }
            }
            RemoveGridLine { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(
                    AddGridLine {
                        id,
                        grid_line: entity,
                    },
                    groups,
                )
            }
            SetGravity { gravity } => {
                if !gravity.si().is_finite() || gravity.si() <= 0.0 {
                    return Err(ModelError::Invalid(
                        "gravity must be positive and finite".into(),
                    ));
                }
                let previous = std::mem::replace(&mut model.gravity, gravity);
                SetGravity { gravity: previous }
            }
            SetMetadata { metadata } => SetMetadata {
                metadata: std::mem::replace(&mut model.metadata, metadata),
            },
            AddMassSource { id, mass_source } => {
                check_mass_source(model, &mass_source)?;
                add(model, id, mass_source)?;
                RemoveMassSource { id }
            }
            UpdateMassSource { id, mass_source } => {
                check_mass_source(model, &mass_source)?;
                UpdateMassSource {
                    id,
                    mass_source: update(model, id, mass_source)?,
                }
            }
            RemoveMassSource { id } => {
                if model.default_mass_source == Some(id) {
                    return Err(ModelError::Invalid(format!(
                        "{} is the default mass source; choose another default first",
                        model.describe(id)
                    )));
                }
                let (entity, groups) = remove(model, id)?;
                removal_inverse(
                    AddMassSource {
                        id,
                        mass_source: entity,
                    },
                    groups,
                )
            }
            SetDefaultMassSource { id } => {
                if let Some(id) = id
                    && !model.mass_sources.contains_key(&id)
                {
                    return Err(match model.kind_of(id) {
                        Some(actual) => ModelError::WrongKind {
                            entity: model.describe(id),
                            expected: EntityKind::MassSource,
                            actual,
                        },
                        None => ModelError::NotFound(id),
                    });
                }
                SetDefaultMassSource {
                    id: std::mem::replace(&mut model.default_mass_source, id),
                }
            }
            Batch { commands } => {
                let mut inverses = vec![];
                for (index, command) in commands.into_iter().enumerate() {
                    match command.apply(model) {
                        Ok(inverse) => inverses.push(inverse),
                        Err(source) => {
                            for inverse in inverses.into_iter().rev() {
                                inverse
                                    .apply(model)
                                    .expect("rollback of an applied command");
                            }
                            return Err(ModelError::Batch {
                                index,
                                source: Box::new(source),
                            });
                        }
                    }
                }
                inverses.reverse();
                Batch { commands: inverses }
            }
        })
    }
}
