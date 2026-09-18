use crate::entity::*;
use oa_core::units::{Acceleration, Length, STANDARD_GRAVITY};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Level,
    Node,
    Material,
    Section,
    Frame,
    Shell,
    Diaphragm,
    LoadCase,
    Combination,
    Group,
}
impl std::fmt::Display for EntityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Level => "level",
            Self::Node => "node",
            Self::Material => "material",
            Self::Section => "section",
            Self::Frame => "frame",
            Self::Shell => "shell",
            Self::Diaphragm => "diaphragm",
            Self::LoadCase => "load case",
            Self::Combination => "combination",
            Self::Group => "group",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
    /// Display preference only. The model is always SI.
    pub display_units: String,
    pub created_unix: u64,
    pub modified_unix: u64,
}

fn standard_gravity() -> Acceleration {
    Acceleration::from_si(STANDARD_GRAVITY)
}
fn format_version() -> u32 {
    crate::format::FORMAT_VERSION
}

/// The editable model. Tables are ordered maps so iteration, and therefore
/// compilation, is deterministic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    #[serde(default = "format_version")]
    pub format_version: u32,
    #[serde(default)]
    pub metadata: Metadata,
    #[serde(default = "standard_gravity")]
    pub gravity: Acceleration,
    #[serde(default)]
    pub next_id: u64,
    /// Never empty: a model keeps at least one level, and every node binds to one.
    #[serde(default)]
    pub levels: BTreeMap<EntityId, Level>,
    #[serde(default)]
    pub nodes: BTreeMap<EntityId, Node>,
    #[serde(default)]
    pub materials: BTreeMap<EntityId, Material>,
    #[serde(default)]
    pub sections: BTreeMap<EntityId, Section>,
    #[serde(default)]
    pub frames: BTreeMap<EntityId, Frame>,
    #[serde(default)]
    pub shells: BTreeMap<EntityId, Shell>,
    #[serde(default)]
    pub diaphragms: BTreeMap<EntityId, Diaphragm>,
    #[serde(default)]
    pub load_cases: BTreeMap<EntityId, LoadCase>,
    #[serde(default)]
    pub combinations: BTreeMap<EntityId, Combination>,
    #[serde(default)]
    pub groups: BTreeMap<EntityId, Group>,
}
/// A new model starts with one level, `Base` at elevation zero, so there is
/// always a datum to bind nodes to.
impl Default for Model {
    fn default() -> Self {
        let mut levels = BTreeMap::new();
        levels.insert(EntityId(1), Level::new("Base", Length::ZERO));
        Self {
            format_version: format_version(),
            metadata: Metadata::default(),
            gravity: standard_gravity(),
            next_id: 2,
            levels,
            nodes: BTreeMap::new(),
            materials: BTreeMap::new(),
            sections: BTreeMap::new(),
            frames: BTreeMap::new(),
            shells: BTreeMap::new(),
            diaphragms: BTreeMap::new(),
            load_cases: BTreeMap::new(),
            combinations: BTreeMap::new(),
            groups: BTreeMap::new(),
        }
    }
}

/// What every entity table has in common, so commands and compilation can be generic.
pub trait Entity: Clone + PartialEq + serde::Serialize + serde::de::DeserializeOwned {
    const KIND: EntityKind;
    fn name(&self) -> &str;
    /// Every entity this one points at, with the kind it must be.
    fn references(&self) -> Vec<(EntityId, EntityKind)>;
    fn table(model: &Model) -> &BTreeMap<EntityId, Self>;
    fn table_mut(model: &mut Model) -> &mut BTreeMap<EntityId, Self>;
}
macro_rules! entity {
    ($t:ty, $kind:ident, $field:ident, |$s:ident| $refs:expr) => {
        impl Entity for $t {
            const KIND: EntityKind = EntityKind::$kind;
            fn name(&self) -> &str {
                &self.name
            }
            fn references(&self) -> Vec<(EntityId, EntityKind)> {
                let $s = self;
                $refs
            }
            fn table(model: &Model) -> &BTreeMap<EntityId, Self> {
                &model.$field
            }
            fn table_mut(model: &mut Model) -> &mut BTreeMap<EntityId, Self> {
                &mut model.$field
            }
        }
    };
}
entity!(Level, Level, levels, |_s| vec![]);
entity!(Node, Node, nodes, |s| vec![(s.level, EntityKind::Level)]);
entity!(Material, Material, materials, |_s| vec![]);
entity!(Section, Section, sections, |_s| vec![]);
entity!(Frame, Frame, frames, |s| vec![
    (s.nodes[0], EntityKind::Node),
    (s.nodes[1], EntityKind::Node),
    (s.material, EntityKind::Material),
    (s.section, EntityKind::Section),
]);
entity!(Shell, Shell, shells, |s| s
    .nodes
    .iter()
    .map(|n| (*n, EntityKind::Node))
    .chain([(s.material, EntityKind::Material)])
    .collect());
entity!(Diaphragm, Diaphragm, diaphragms, |s| s
    .master
    .iter()
    .chain(s.nodes.iter())
    .map(|n| (*n, EntityKind::Node))
    .collect());
entity!(LoadCase, LoadCase, load_cases, |s| s
    .nodal
    .iter()
    .map(|l| (l.node, EntityKind::Node))
    .chain(s.member.iter().map(|l| (l.member(), EntityKind::Frame)))
    .chain(s.surface.iter().map(|l| (l.shell, EntityKind::Shell)))
    .collect());
entity!(Combination, Combination, combinations, |s| s
    .terms
    .iter()
    .map(|(id, _)| (*id, EntityKind::LoadCase))
    .collect());
impl Entity for Group {
    const KIND: EntityKind = EntityKind::Group;
    fn name(&self) -> &str {
        &self.name
    }
    /// Groups may hold any kind; membership is checked against existence only.
    fn references(&self) -> Vec<(EntityId, EntityKind)> {
        vec![]
    }
    fn table(model: &Model) -> &BTreeMap<EntityId, Self> {
        &model.groups
    }
    fn table_mut(model: &mut Model) -> &mut BTreeMap<EntityId, Self> {
        &mut model.groups
    }
}

impl Model {
    /// Hands out the next unused id and reserves it.
    ///
    /// The counter saturates rather than wrapping: at exhaustion the same id
    /// comes back and command validation rejects it as in use, instead of a
    /// wrapped id silently replacing an unrelated entity.
    pub fn allocate(&mut self) -> EntityId {
        let id = EntityId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        id
    }
    pub(crate) fn reserve(&mut self, id: EntityId) {
        if id.0 >= self.next_id {
            self.next_id = id.0.saturating_add(1);
        }
    }
    fn all_ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.levels
            .keys()
            .chain(self.nodes.keys())
            .chain(self.materials.keys())
            .chain(self.sections.keys())
            .chain(self.frames.keys())
            .chain(self.shells.keys())
            .chain(self.diaphragms.keys())
            .chain(self.load_cases.keys())
            .chain(self.combinations.keys())
            .chain(self.groups.keys())
            .copied()
    }
    /// The highest id any table holds.
    pub fn max_id(&self) -> Option<EntityId> {
        self.all_ids().max()
    }
    /// Ids that more than one table holds. Identity is model-wide, so any
    /// such id makes `kind_of` and every id-based lookup ambiguous.
    pub fn duplicate_ids(&self) -> Vec<EntityId> {
        let mut seen = std::collections::BTreeSet::new();
        let mut duplicates = std::collections::BTreeSet::new();
        for id in self.all_ids() {
            if !seen.insert(id) {
                duplicates.insert(id);
            }
        }
        duplicates.into_iter().collect()
    }
    /// Group members that no table holds, as (group, member).
    pub fn dangling_group_members(&self) -> Vec<(EntityId, EntityId)> {
        self.groups
            .iter()
            .flat_map(|(group, g)| {
                g.members
                    .iter()
                    .filter(|m| self.kind_of(**m).is_none())
                    .map(move |m| (*group, *m))
            })
            .collect()
    }
    pub fn kind_of(&self, id: EntityId) -> Option<EntityKind> {
        [
            (self.levels.contains_key(&id), EntityKind::Level),
            (self.nodes.contains_key(&id), EntityKind::Node),
            (self.materials.contains_key(&id), EntityKind::Material),
            (self.sections.contains_key(&id), EntityKind::Section),
            (self.frames.contains_key(&id), EntityKind::Frame),
            (self.shells.contains_key(&id), EntityKind::Shell),
            (self.diaphragms.contains_key(&id), EntityKind::Diaphragm),
            (self.load_cases.contains_key(&id), EntityKind::LoadCase),
            (self.combinations.contains_key(&id), EntityKind::Combination),
            (self.groups.contains_key(&id), EntityKind::Group),
        ]
        .into_iter()
        .find(|(present, _)| *present)
        .map(|(_, k)| k)
    }
    pub fn name_of(&self, id: EntityId) -> Option<&str> {
        self.levels
            .get(&id)
            .map(|e| e.name.as_str())
            .or_else(|| self.nodes.get(&id).map(|e| e.name.as_str()))
            .or_else(|| self.materials.get(&id).map(|e| e.name.as_str()))
            .or_else(|| self.sections.get(&id).map(|e| e.name.as_str()))
            .or_else(|| self.frames.get(&id).map(|e| e.name.as_str()))
            .or_else(|| self.shells.get(&id).map(|e| e.name.as_str()))
            .or_else(|| self.diaphragms.get(&id).map(|e| e.name.as_str()))
            .or_else(|| self.load_cases.get(&id).map(|e| e.name.as_str()))
            .or_else(|| self.combinations.get(&id).map(|e| e.name.as_str()))
            .or_else(|| self.groups.get(&id).map(|e| e.name.as_str()))
    }
    /// Human-readable handle for messages: `node "N7" (#12)`.
    pub fn describe(&self, id: EntityId) -> String {
        match (self.kind_of(id), self.name_of(id)) {
            (Some(k), Some(n)) => format!("{k} {n:?} (#{})", id.0),
            _ => format!("unknown entity #{}", id.0),
        }
    }
    /// Look an entity up by kind and name.
    pub fn find<T: Entity>(&self, name: &str) -> Option<EntityId> {
        T::table(self)
            .iter()
            .find(|(_, e)| e.name() == name)
            .map(|(id, _)| *id)
    }
    /// Entities that reference `id`, excluding groups.
    pub fn referrers(&self, id: EntityId) -> Vec<EntityId> {
        fn scan<T: Entity>(model: &Model, id: EntityId, out: &mut Vec<EntityId>) {
            for (owner, e) in T::table(model) {
                if e.references().iter().any(|(r, _)| *r == id) {
                    out.push(*owner);
                }
            }
        }
        let mut out = vec![];
        scan::<Node>(self, id, &mut out);
        scan::<Frame>(self, id, &mut out);
        scan::<Shell>(self, id, &mut out);
        scan::<Diaphragm>(self, id, &mut out);
        scan::<LoadCase>(self, id, &mut out);
        scan::<Combination>(self, id, &mut out);
        out
    }
    /// Existing members of a group. Stale ids are skipped.
    pub fn group_members(&self, group: EntityId) -> Vec<EntityId> {
        self.groups
            .get(&group)
            .map(|g| {
                g.members
                    .iter()
                    .copied()
                    .filter(|m| self.kind_of(*m).is_some())
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Convenience constructors that allocate an id. They bypass command
    /// validation and exist for tests and importers; editing goes through commands.
    pub fn insert<T: Entity>(&mut self, entity: T) -> EntityId {
        let id = self.allocate();
        T::table_mut(self).insert(id, entity);
        id
    }

    /// Levels from the lowest up. Elevations are distinct (validation
    /// rejects coincident datums), so id order only breaks exact ties.
    pub fn levels_by_elevation(&self) -> Vec<EntityId> {
        let mut ids: Vec<(f64, EntityId)> = self
            .levels
            .iter()
            .map(|(id, l)| (l.elevation.si(), *id))
            .collect();
        ids.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        ids.into_iter().map(|(_, id)| id).collect()
    }
    /// The lowest level, if the model has any.
    pub fn base_level(&self) -> Option<EntityId> {
        self.levels_by_elevation().into_iter().next()
    }

    /// Builds a model from solver input, naming entities by their table
    /// position. Solver input has no levels, so every node binds to the
    /// default model's base level at elevation zero, with its Z as the offset.
    pub fn from_solver(solver: &oa_core::Model) -> Self {
        let mut m = Model {
            gravity: solver.gravity,
            ..Default::default()
        };
        let base = m.base_level().expect("a new model has a level");
        let nodes: Vec<EntityId> = solver
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                m.insert(Node {
                    name: format!("N{i}"),
                    level: base,
                    position: n.position,
                    restrained: n.restrained,
                    prescribed: n.prescribed.clone(),
                    mass: n.mass,
                    mass_inertia: n.mass_inertia,
                    spring_translation: n.spring_translation,
                    spring_rotation: n.spring_rotation,
                })
            })
            .collect();
        let materials: Vec<EntityId> = solver
            .materials
            .iter()
            .enumerate()
            .map(|(i, x)| {
                m.insert(Material {
                    name: format!("M{i}"),
                    young: x.young,
                    poisson: x.poisson,
                    density: x.density,
                    provenance: None,
                })
            })
            .collect();
        let sections: Vec<EntityId> = solver
            .sections
            .iter()
            .enumerate()
            .map(|(i, x)| {
                m.insert(Section {
                    name: format!("S{i}"),
                    area: x.area,
                    iy: x.iy,
                    iz: x.iz,
                    torsion: x.torsion,
                    provenance: None,
                })
            })
            .collect();
        let frames: Vec<EntityId> = solver
            .frames
            .iter()
            .enumerate()
            .map(|(i, f)| {
                m.insert(Frame {
                    name: format!("F{i}"),
                    nodes: [nodes[f.nodes[0].0], nodes[f.nodes[1].0]],
                    material: materials[f.material.0],
                    section: sections[f.section.0],
                    local_y: f.local_y,
                    roll: f.roll,
                    releases: f.releases,
                    behavior: f.behavior,
                })
            })
            .collect();
        let shells: Vec<EntityId> = solver
            .shells
            .iter()
            .enumerate()
            .map(|(i, s)| {
                m.insert(Shell {
                    name: format!("SH{i}"),
                    nodes: s.nodes.map(|n| nodes[n.0]),
                    material: materials[s.material.0],
                    thickness: s.thickness,
                    formulation: s.formulation,
                    drilling_ratio: s.drilling_ratio,
                })
            })
            .collect();
        for (i, d) in solver.diaphragms.iter().enumerate() {
            m.insert(Diaphragm {
                name: format!("D{i}"),
                master: Some(nodes[d.master.0]),
                nodes: d.nodes.iter().map(|n| nodes[n.0]).collect(),
                normal: d.normal,
            });
        }
        let cases: Vec<EntityId> = solver
            .load_cases
            .iter()
            .map(|c| {
                m.insert(LoadCase {
                    name: c.name.clone(),
                    load_type: LoadType::Other,
                    self_weight: c.self_weight,
                    nodal: c
                        .nodal
                        .iter()
                        .map(|l| NodalLoad {
                            node: nodes[l.node.0],
                            force: l.force,
                            moment: l.moment,
                        })
                        .collect(),
                    member: c
                        .member
                        .iter()
                        .map(|l| match l {
                            oa_core::MemberLoad::Point {
                                member,
                                position,
                                force,
                                moment,
                                axes,
                            } => MemberLoad::Point {
                                member: frames[member.0],
                                position: *position,
                                force: *force,
                                moment: *moment,
                                axes: *axes,
                            },
                            oa_core::MemberLoad::Distributed {
                                member,
                                start,
                                end,
                                start_load,
                                end_load,
                                axes,
                            } => MemberLoad::Distributed {
                                member: frames[member.0],
                                start: *start,
                                end: *end,
                                start_load: *start_load,
                                end_load: *end_load,
                                axes: *axes,
                            },
                        })
                        .collect(),
                    surface: c
                        .surface
                        .iter()
                        .map(|l| SurfaceLoad {
                            shell: shells[l.shell.0],
                            pressure: l.pressure,
                        })
                        .collect(),
                })
            })
            .collect();
        for c in &solver.combinations {
            m.insert(Combination {
                name: c.name.clone(),
                terms: c.terms.iter().map(|(id, f)| (cases[id.0], *f)).collect(),
            });
        }
        m
    }
}
