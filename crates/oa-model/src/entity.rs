//! Model-layer entities. They mirror the solver's types but reference each
//! other by [`EntityId`] instead of table position, and carry a name.
use oa_core::units::*;
pub use oa_core::{
    Axes, AxialBehavior, Axis, FrameModifiers, FrameOffsets, PrescribedDisplacement,
    ShellFormulation, ShellModifiers,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
    /// Specified minimum yield stress, for steel. The solver never reads
    /// the strengths; design checks do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fy: Option<Pressure>,
    /// Specified minimum tensile strength, for steel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fu: Option<Pressure>,
    /// Specified compressive strength, f'c, for concrete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fc: Option<Pressure>,
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
    /// Effective shear areas for shear along local y (bending with `iz`) and
    /// local z (bending with `iy`). Absent means rigid in shear, so that
    /// plane bends as an Euler-Bernoulli beam.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shear_y: Option<Area>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shear_z: Option<Area>,
    /// Design properties, present when the section was copied from a steel
    /// shape table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<Shape>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
}

/// A rolled or built-up steel shape's type and design properties, as the
/// AISC Shapes Database tabulates them. Properties a shape type does not
/// have are absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    pub kind: ShapeKind,
    pub properties: BTreeMap<SectionProperty, f64>,
}

/// The AISC shape types a frame section can be. Single and double angles
/// are not among them: a single angle bends about inclined principal axes,
/// and the AISC table gives double angles no torsion constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    W,
    M,
    S,
    HP,
    C,
    MC,
    WT,
    MT,
    ST,
    /// Rectangular, square, and round HSS.
    HSS,
    PIPE,
}

/// A design property, named as in the AISC Shapes Database. `x` is the axis
/// of the section's `iz` (AISC Ix) and `y` that of its `iy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SectionProperty {
    /// Overall depth.
    #[serde(rename = "d")]
    Depth,
    /// Overall depth of square HSS, or the longer wall of rectangular HSS.
    #[serde(rename = "Ht")]
    HssDepth,
    /// Flat depth of the longer HSS wall.
    #[serde(rename = "h")]
    HssFlatDepth,
    /// Outside diameter of round HSS and pipe.
    #[serde(rename = "OD")]
    OutsideDiameter,
    /// Flange width.
    #[serde(rename = "bf")]
    FlangeWidth,
    /// Overall width of square HSS, or the shorter wall of rectangular HSS.
    #[serde(rename = "B")]
    HssWidth,
    /// Flat width of the shorter HSS wall.
    #[serde(rename = "b")]
    HssFlatWidth,
    /// Inside diameter of pipe.
    #[serde(rename = "ID")]
    InsideDiameter,
    #[serde(rename = "tw")]
    WebThickness,
    #[serde(rename = "tf")]
    FlangeThickness,
    /// Nominal HSS and pipe wall thickness.
    #[serde(rename = "tnom")]
    NominalWallThickness,
    /// Design HSS and pipe wall thickness.
    #[serde(rename = "tdes")]
    DesignWallThickness,
    /// Outer face of flange to web toe of fillet, for design.
    #[serde(rename = "kdes")]
    Kdes,
    /// Horizontal distance from the designated edge to the centroid.
    #[serde(rename = "x")]
    CentroidX,
    /// Vertical distance from the designated edge to the centroid.
    #[serde(rename = "y")]
    CentroidY,
    /// Horizontal distance from the designated edge to the shear centre.
    #[serde(rename = "eo")]
    ShearCentre,
    /// Horizontal distance from the designated edge to the plastic neutral axis.
    #[serde(rename = "xp")]
    PlasticAxisX,
    /// Vertical distance from the designated edge to the plastic neutral axis.
    #[serde(rename = "yp")]
    PlasticAxisY,
    #[serde(rename = "bf/2tf")]
    FlangeSlenderness,
    /// Angle leg or channel flange slenderness.
    #[serde(rename = "b/t")]
    LegSlenderness,
    #[serde(rename = "b/tdes")]
    HssWidthSlenderness,
    #[serde(rename = "h/tw")]
    WebSlenderness,
    #[serde(rename = "h/tdes")]
    HssDepthSlenderness,
    /// Round HSS and pipe slenderness, or tee stem slenderness.
    #[serde(rename = "D/t")]
    DiameterSlenderness,
    #[serde(rename = "Zx")]
    Zx,
    #[serde(rename = "Sx")]
    Sx,
    #[serde(rename = "rx")]
    Rx,
    #[serde(rename = "Zy")]
    Zy,
    #[serde(rename = "Sy")]
    Sy,
    #[serde(rename = "ry")]
    Ry,
    /// Warping constant.
    #[serde(rename = "Cw")]
    Cw,
    /// HSS torsional constant, a modulus.
    #[serde(rename = "C")]
    HssTorsionalConstant,
    /// Polar radius of gyration about the shear centre.
    #[serde(rename = "ro")]
    Ro,
    /// Flexural constant.
    #[serde(rename = "H")]
    FlexuralConstant,
    /// Effective radius of gyration.
    #[serde(rename = "rts")]
    Rts,
    /// Distance between flange centroids.
    #[serde(rename = "ho")]
    Ho,
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
    #[serde(default, skip_serializing_if = "FrameModifiers::is_unmodified")]
    pub modifiers: FrameModifiers,
    /// Joint offsets, end length offsets and the rigid zone factor, as the
    /// solver takes them; see [`FrameOffsets`].
    #[serde(default, skip_serializing_if = "FrameOffsets::is_none")]
    pub offsets: FrameOffsets,
    /// The point of the section that sits on the line between the nodes.
    /// Compilation turns it into joint offsets from the section's steel
    /// shape and adds them to `offsets.joint`.
    #[serde(default, skip_serializing_if = "CardinalPoint::is_centroid")]
    pub cardinal_point: CardinalPoint,
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
            modifiers: FrameModifiers::default(),
            offsets: FrameOffsets::default(),
            cardinal_point: CardinalPoint::Centroid,
        }
    }
}

/// The point of a frame's section placed on the line between its nodes,
/// or between the ends its joint offsets move it to, numbered 1 to 10 as in
/// ETABS and SAP2000. The section is seen from end I looking toward J, with
/// local y up, so top is +y and right is +z, in the axes of that line. The
/// bounding box is the steel shape's depth along y and width along z. A
/// channel's web is at the left and a tee's flange at the top, the way the
/// shape tables measure their centroids. A beam whose nodes sit at the top
/// of the slab takes `TopCenter` and hangs below them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardinalPoint {
    BottomLeft,
    BottomCenter,
    BottomRight,
    MiddleLeft,
    MiddleCenter,
    MiddleRight,
    TopLeft,
    TopCenter,
    TopRight,
    #[default]
    Centroid,
}
impl CardinalPoint {
    pub const ALL: [Self; 10] = [
        Self::BottomLeft,
        Self::BottomCenter,
        Self::BottomRight,
        Self::MiddleLeft,
        Self::MiddleCenter,
        Self::MiddleRight,
        Self::TopLeft,
        Self::TopCenter,
        Self::TopRight,
        Self::Centroid,
    ];
    pub fn is_centroid(&self) -> bool {
        *self == Self::Centroid
    }
    /// ETABS's number for it, 1 to 10.
    pub fn number(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(9) + 1
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::BottomLeft => "Bottom left",
            Self::BottomCenter => "Bottom center",
            Self::BottomRight => "Bottom right",
            Self::MiddleLeft => "Middle left",
            Self::MiddleCenter => "Middle center",
            Self::MiddleRight => "Middle right",
            Self::TopLeft => "Top left",
            Self::TopCenter => "Top center",
            Self::TopRight => "Top right",
            Self::Centroid => "Centroid",
        }
    }
    /// Where the section's centroid sits relative to this point, in local y
    /// and z, metres; None when the section has no shape to measure.
    pub fn centroid_offset(self, section: &Section) -> Option<[f64; 2]> {
        if self.is_centroid() {
            return Some([0.0; 2]);
        }
        let shape = section.shape.as_ref()?;
        let p = |property| shape.properties.get(&property).copied();
        use SectionProperty::*;
        // Bounding box, then the centroid measured from its bottom left.
        let (depth, width) = if let Some(od) = p(OutsideDiameter) {
            (od, od)
        } else if let (Some(d), Some(b)) = (p(HssDepth), p(HssWidth)) {
            (d, b)
        } else {
            (p(Depth)?, p(FlangeWidth)?)
        };
        let (cy, cz) = match shape.kind {
            ShapeKind::C | ShapeKind::MC => (depth / 2.0, p(CentroidX)?),
            ShapeKind::WT | ShapeKind::MT | ShapeKind::ST => (depth - p(CentroidY)?, width / 2.0),
            _ => (depth / 2.0, width / 2.0),
        };
        let index = self.number() - 1;
        let y = [0.0, depth / 2.0, depth][index / 3];
        let z = [0.0, width / 2.0, width][index % 3];
        Some([cy - y, cz - z])
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
    /// Reference for local +x, projected into the shell's plane; None puts
    /// local x along the edge from the first corner to the second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_x: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "ShellModifiers::is_unmodified")]
    pub modifiers: ShellModifiers,
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

fn yes() -> bool {
    true
}
/// A named definition of what modal and spectrum analysis take as mass, as
/// in ETABS. Node mass always counts; `element_mass` adds the members' own
/// mass from density, and each listed load case adds its gravity load
/// divided by g, times its multiplier. `lateral` keeps X and Y translation
/// and rotation about Z, `vertical` keeps Z translation and rotation about X
/// and Y, and `lump_to_levels` moves lateral mass between levels onto the
/// levels. The model's default source is the one analysis uses unless told
/// otherwise; the solver's [`oa_core::MassSource`] has the details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MassSource {
    pub name: String,
    #[serde(default = "yes")]
    pub element_mass: bool,
    #[serde(default)]
    pub cases: Vec<(EntityId, f64)>,
    #[serde(default = "yes")]
    pub lateral: bool,
    #[serde(default = "yes")]
    pub vertical: bool,
    #[serde(default)]
    pub lump_to_levels: bool,
}
impl MassSource {
    /// Element and node mass in every direction, and no load cases: what
    /// analysis uses when the model has no default source.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            element_mass: true,
            cases: vec![],
            lateral: true,
            vertical: true,
            lump_to_levels: false,
        }
    }
    /// The multiplier on a load case, if the source lists it.
    pub fn multiplier(&self, case: EntityId) -> Option<f64> {
        self.cases.iter().find(|(c, _)| *c == case).map(|(_, f)| *f)
    }
}

/// Named set of entities of any kind. Membership is validated on edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub name: String,
    #[serde(default)]
    pub members: BTreeSet<EntityId>,
}

/// A CAD drawing flattened to line segments and laid on a level's plane as a
/// reference to draw over. It is never compiled: the solver does not see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Underlay {
    pub name: String,
    /// The level whose plane the drawing lies on; it follows the datum.
    pub level: EntityId,
    /// Where the drawing's own origin sits in plan.
    #[serde(default)]
    pub origin: [Length; 2],
    /// Segment ends in plan, relative to `origin`.
    pub segments: Vec<[[Length; 2]; 2]>,
}

/// A plan grid line, as in ETABS: a labelled line in plan that stands for a
/// vertical plane through every level. The GUI draws it with its bubble on
/// the level in view, and the draw tools snap to it and to where grid lines
/// cross. It is a reference only: the solver never sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridLine {
    /// The label in the bubble, such as "A" or "3".
    pub name: String,
    /// The bubble end, in plan.
    pub start: [Length; 2],
    pub end: [Length; 2],
}
