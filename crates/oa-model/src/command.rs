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
    SetGravity {
        gravity: Acceleration,
    },
    SetMetadata {
        metadata: Metadata,
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
                add(model, id, material)?;
                RemoveMaterial { id }
            }
            UpdateMaterial { id, material } => UpdateMaterial {
                id,
                material: update(model, id, material)?,
            },
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
                add(model, id, section)?;
                RemoveSection { id }
            }
            UpdateSection { id, section } => UpdateSection {
                id,
                section: update(model, id, section)?,
            },
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
                add(model, id, frame)?;
                RemoveFrame { id }
            }
            UpdateFrame { id, frame } => UpdateFrame {
                id,
                frame: update(model, id, frame)?,
            },
            RemoveFrame { id } => {
                let (entity, groups) = remove(model, id)?;
                removal_inverse(AddFrame { id, frame: entity }, groups)
            }
            AddShell { id, shell } => {
                add(model, id, shell)?;
                RemoveShell { id }
            }
            UpdateShell { id, shell } => UpdateShell {
                id,
                shell: update(model, id, shell)?,
            },
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
