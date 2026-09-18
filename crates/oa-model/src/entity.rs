//! Model-layer entities. They mirror the solver's types but reference each
//! other by [`EntityId`] instead of table position, and carry a name.
use oa_core::units::*;
pub use oa_core::{Axes, AxialBehavior, Axis, PrescribedDisplacement, ShellFormulation};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Allocated once per model and never reused, across every entity kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub u64);

/// Where a copied library entry came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub library: String,
    pub version: String,
    pub designation: String,
}

/// A named horizontal datum at an absolute global Z elevation. Every node
/// binds to exactly one; the node's height above it is derived from its
/// position, never stored a second time. Z is the structural vertical.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Level {
    pub name: String,
    pub elevation: Length,
}
impl Level {
    pub fn new(name: impl Into<String>, elevation: Length) -> Self {
        Self {
            name: name.into(),
            elevation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub name: String,
    /// The level this node follows when its datum moves.
    pub level: EntityId,
    pub position: [Length; 3],
    #[serde(default)]
    pub restrained: [bool; 6],
    #[serde(default)]
    pub prescribed: PrescribedDisplacement,
    #[serde(default)]
    pub mass: [Mass; 3],
    #[serde(default)]
    pub mass_inertia: [MassInertia; 3],
    #[serde(default)]
    pub spring_translation: [Stiffness; 3],
    #[serde(default)]
    pub spring_rotation: [RotationalStiffness; 3],
}
impl Node {
    pub fn new(name: impl Into<String>, level: EntityId, position: [Length; 3]) -> Self {
        Self {
            name: name.into(),
            level,
            position,
            restrained: [false; 6],
            prescribed: Default::default(),
            mass: [Mass::ZERO; 3],
            mass_inertia: [MassInertia::ZERO; 3],
            spring_translation: [Stiffness::ZERO; 3],
            spring_rotation: [RotationalStiffness::ZERO; 3],
        }
    }
    pub fn fixed(name: impl Into<String>, level: EntityId, position: [Length; 3]) -> Self {
        Self {
            restrained: [true; 6],
            ..Self::new(name, level, position)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub name: String,
    pub young: Pressure,
    pub poisson: f64,
    #[serde(default)]
    pub density: MassDensity,
    #[serde(default)]
    pub provenance: Option<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub name: String,
    pub area: Area,
    pub iy: SecondMoment,
    pub iz: SecondMoment,
    pub torsion: SecondMoment,
    #[serde(default)]
    pub provenance: Option<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub name: String,
    pub nodes: [EntityId; 2],
    pub material: EntityId,
    pub section: EntityId,
    #[serde(default)]
    pub local_y: Option<[f64; 3]>,
    #[serde(default)]
    pub roll: Angle,
    #[serde(default)]
    pub releases: [bool; 12],
    #[serde(default)]
    pub behavior: AxialBehavior,
}
impl Frame {
    pub fn new(
        name: impl Into<String>,
        nodes: [EntityId; 2],
        material: EntityId,
        section: EntityId,
    ) -> Self {
        Self {
            name: name.into(),
            nodes,
            material,
            section,
            local_y: None,
            roll: Angle::ZERO,
            releases: [false; 12],
            behavior: AxialBehavior::Both,
        }
    }
}

fn default_drilling() -> f64 {
    1e-3
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shell {
    pub name: String,
    pub nodes: [EntityId; 4],
    pub material: EntityId,
    pub thickness: Length,
    #[serde(default)]
    pub formulation: ShellFormulation,
    #[serde(default = "default_drilling")]
    pub drilling_ratio: f64,
}

/// Rigid diaphragm. With `master: None`, compilation creates a master node at
/// the mass-weighted centroid of the slaves (plain centroid when massless).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diaphragm {
    pub name: String,
    #[serde(default)]
    pub master: Option<EntityId>,
    pub nodes: Vec<EntityId>,
    pub normal: Axis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodalLoad {
    pub node: EntityId,
    #[serde(default)]
    pub force: [Force; 3],
    #[serde(default)]
    pub moment: [Moment; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberLoad {
    Point {
        member: EntityId,
        position: Length,
        #[serde(default)]
        force: [Force; 3],
        #[serde(default)]
        moment: [Moment; 3],
        #[serde(default)]
        axes: Axes,
    },
    Distributed {
        member: EntityId,
        start: Length,
        end: Length,
        start_load: [LineLoad; 3],
        end_load: [LineLoad; 3],
        #[serde(default)]
        axes: Axes,
    },
}
impl MemberLoad {
    pub fn member(&self) -> EntityId {
        match self {
            Self::Point { member, .. } | Self::Distributed { member, .. } => *member,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceLoad {
    pub shell: EntityId,
    pub pressure: Pressure,
}

/// The nominal load a case represents, in the vocabulary of ASCE 7 chapter 2.
/// Combination generators pick cases by it; the solver never reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadType {
    Dead,
    Live,
    RoofLive,
    Snow,
    Rain,
    Wind,
    Earthquake,
    EarthPressure,
    Fluid,
    SelfStraining,
    Flood,
    Ice,
    WindOnIce,
    /// A case of the user's own that no generator touches.
    #[default]
    Other,
}
impl LoadType {
    /// Every type, in the order ASCE 7 lists them, with `Other` last.
    pub const ALL: [LoadType; 14] = [
        LoadType::Dead,
        LoadType::Live,
        LoadType::RoofLive,
        LoadType::Snow,
        LoadType::Rain,
        LoadType::Wind,
        LoadType::Earthquake,
        LoadType::EarthPressure,
        LoadType::Fluid,
        LoadType::SelfStraining,
        LoadType::Flood,
        LoadType::Ice,
        LoadType::WindOnIce,
        LoadType::Other,
    ];
    /// The chapter 2 symbol, empty for `Other`.
    pub fn symbol(self) -> &'static str {
        match self {
            LoadType::Dead => "D",
            LoadType::Live => "L",
            LoadType::RoofLive => "Lr",
            LoadType::Snow => "S",
            LoadType::Rain => "R",
            LoadType::Wind => "W",
            LoadType::Earthquake => "E",
            LoadType::EarthPressure => "H",
            LoadType::Fluid => "F",
            LoadType::SelfStraining => "T",
            LoadType::Flood => "Fa",
            LoadType::Ice => "Di",
            LoadType::WindOnIce => "Wi",
            LoadType::Other => "",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            LoadType::Dead => "Dead",
            LoadType::Live => "Live",
            LoadType::RoofLive => "Roof live",
            LoadType::Snow => "Snow",
            LoadType::Rain => "Rain",
            LoadType::Wind => "Wind",
            LoadType::Earthquake => "Earthquake",
            LoadType::EarthPressure => "Earth pressure",
            LoadType::Fluid => "Fluid",
            LoadType::SelfStraining => "Self-straining",
            LoadType::Flood => "Flood",
            LoadType::Ice => "Ice",
            LoadType::WindOnIce => "Wind on ice",
            LoadType::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadCase {
    pub name: String,
    #[serde(default)]
    pub load_type: LoadType,
    #[serde(default)]
    pub self_weight: [f64; 3],
    #[serde(default)]
    pub nodal: Vec<NodalLoad>,
    #[serde(default)]
    pub member: Vec<MemberLoad>,
    #[serde(default)]
    pub surface: Vec<SurfaceLoad>,
}
impl LoadCase {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            load_type: LoadType::Other,
            self_weight: [0.0; 3],
            nodal: vec![],
            member: vec![],
            surface: vec![],
        }
    }
    pub fn with_type(mut self, load_type: LoadType) -> Self {
        self.load_type = load_type;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Combination {
    pub name: String,
    pub terms: Vec<(EntityId, f64)>,
}

/// Named set of entities of any kind. Membership is validated on edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    #[serde(default)]
    pub members: BTreeSet<EntityId>,
}
