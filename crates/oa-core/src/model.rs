use crate::{Error, Result, units::*};
use nalgebra::Vector3;
use serde::{Deserialize, Serialize};

macro_rules! id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub usize);
    };
}
id!(NodeId);
id!(MaterialId);
id!(SectionId);
id!(FrameId);
id!(ShellId);
id!(LoadCaseId);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrescribedDisplacement {
    pub translation: [Length; 3],
    pub rotation: [Angle; 3],
}
impl PrescribedDisplacement {
    pub fn values(&self) -> [f64; 6] {
        [
            self.translation[0].si(),
            self.translation[1].si(),
            self.translation[2].si(),
            self.rotation[0].si(),
            self.rotation[1].si(),
            self.rotation[2].si(),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub position: [Length; 3],
    #[serde(default)]
    pub restrained: [bool; 6],
    #[serde(default)]
    pub prescribed: PrescribedDisplacement,
    #[serde(default)]
    pub mass: [Mass; 3],
    #[serde(default)]
    pub mass_inertia: [MassInertia; 3],
    /// Grounded springs added to the diagonal stiffness. Zero disables. A spring
    /// on a restrained DOF is an error; restraints already fix that DOF.
    #[serde(default)]
    pub spring_translation: [Stiffness; 3],
    #[serde(default)]
    pub spring_rotation: [RotationalStiffness; 3],
}
impl Node {
    pub fn new(position: [Length; 3]) -> Self {
        Self {
            position,
            restrained: [false; 6],
            prescribed: Default::default(),
            mass: [Mass::ZERO; 3],
            mass_inertia: [MassInertia::ZERO; 3],
            spring_translation: [Stiffness::ZERO; 3],
            spring_rotation: [RotationalStiffness::ZERO; 3],
        }
    }
    pub fn springs(&self) -> [f64; 6] {
        [
            self.spring_translation[0].si(),
            self.spring_translation[1].si(),
            self.spring_translation[2].si(),
            self.spring_rotation[0].si(),
            self.spring_rotation[1].si(),
            self.spring_rotation[2].si(),
        ]
    }
    pub fn fixed(position: [Length; 3]) -> Self {
        Self {
            restrained: [true; 6],
            ..Self::new(position)
        }
    }
    pub fn xyz(&self) -> [f64; 3] {
        self.position.map(Length::si)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub young: Pressure,
    pub poisson: f64,
    #[serde(default)]
    pub density: MassDensity,
}
impl Material {
    pub fn shear_modulus(&self) -> f64 {
        self.young.si() / (2.0 * (1.0 + self.poisson))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub area: Area,
    pub iy: SecondMoment,
    pub iz: SecondMoment,
    pub torsion: SecondMoment,
    /// Effective shear areas for shear along local y (bending with `iz`) and
    /// local z (bending with `iy`). Absent means rigid in shear: that plane
    /// bends as an Euler-Bernoulli beam.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shear_y: Option<Area>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shear_z: Option<Area>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxialBehavior {
    #[default]
    Both,
    TensionOnly,
    CompressionOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub nodes: [NodeId; 2],
    pub material: MaterialId,
    pub section: SectionId,
    /// Reference for local +y. Projected normal to the member axis.
    #[serde(default)]
    pub local_y: Option<[f64; 3]>,
    #[serde(default)]
    pub roll: Angle,
    /// [ux,uy,uz,rx,ry,rz] at i, then j, in member coordinates.
    #[serde(default)]
    pub releases: [bool; 12],
    #[serde(default)]
    pub behavior: AxialBehavior,
    #[serde(default, skip_serializing_if = "FrameModifiers::is_unmodified")]
    pub modifiers: FrameModifiers,
}
impl Frame {
    pub fn new(nodes: [NodeId; 2], material: MaterialId, section: SectionId) -> Self {
        Self {
            nodes,
            material,
            section,
            local_y: None,
            roll: Angle::ZERO,
            releases: [false; 12],
            behavior: AxialBehavior::Both,
            modifiers: FrameModifiers::default(),
        }
    }
}

/// Property modifiers for one frame, as ETABS and SAP2000 assign them. The
/// first six multiply the section property they name in the stiffness only.
/// ACI 318 cracked sections, for example, take 0.35 on iy and iz for beams
/// and 0.70 for columns. `mass` multiplies the member's own mass and
/// `weight` its self-weight, which a load case's self-weight factor applies
/// and which a mass source case turns back into mass; zero on both leaves
/// out a member whose mass and weight another member already carries.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FrameModifiers {
    pub area: f64,
    pub shear_y: f64,
    pub shear_z: f64,
    pub torsion: f64,
    pub iy: f64,
    pub iz: f64,
    pub mass: f64,
    pub weight: f64,
}
impl Default for FrameModifiers {
    fn default() -> Self {
        Self {
            area: 1.0,
            shear_y: 1.0,
            shear_z: 1.0,
            torsion: 1.0,
            iy: 1.0,
            iz: 1.0,
            mass: 1.0,
            weight: 1.0,
        }
    }
}
impl FrameModifiers {
    pub fn is_unmodified(&self) -> bool {
        *self == Self::default()
    }
    /// The stiffness modifiers, which must be positive and finite.
    pub fn values(&self) -> [f64; 6] {
        [
            self.area,
            self.shear_y,
            self.shear_z,
            self.torsion,
            self.iy,
            self.iz,
        ]
    }
}

/// Stiffness modifiers for one shell, in its local axes. `membrane_x` and
/// `membrane_y` scale the in-plane normal stiffness along local x and y (f11
/// and f22 in ETABS), `membrane_shear` the in-plane shear stiffness (f12),
/// and `bending` the plate bending stiffness (m11, m22 and m12). The
/// plane-stress matrix is scaled as S D S with S = diag(√fx, √fy, √f12), so
/// the coupling term takes √(fx fy) and the matrix stays positive definite.
/// Transverse shear is unchanged. `mass` and `weight` act as on frames.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellModifiers {
    pub membrane_x: f64,
    pub membrane_y: f64,
    pub membrane_shear: f64,
    pub bending: f64,
    pub mass: f64,
    pub weight: f64,
}
impl Default for ShellModifiers {
    fn default() -> Self {
        Self {
            membrane_x: 1.0,
            membrane_y: 1.0,
            membrane_shear: 1.0,
            bending: 1.0,
            mass: 1.0,
            weight: 1.0,
        }
    }
}
impl ShellModifiers {
    pub fn is_unmodified(&self) -> bool {
        *self == Self::default()
    }
    /// The stiffness modifiers, which must be positive and finite.
    pub fn values(&self) -> [f64; 4] {
        [
            self.membrane_x,
            self.membrane_y,
            self.membrane_shear,
            self.bending,
        ]
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellFormulation {
    Rectangular,
    #[default]
    Dkmq,
}

fn default_drilling() -> f64 {
    1e-3
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shell {
    /// Four coplanar corners, counterclockwise viewed from local +z.
    pub nodes: [NodeId; 4],
    pub material: MaterialId,
    pub thickness: Length,
    #[serde(default)]
    pub formulation: ShellFormulation,
    #[serde(default = "default_drilling")]
    pub drilling_ratio: f64,
    /// Reference for local +x, projected into the shell's plane; local y is
    /// z × x. None puts local x along the edge from the first corner to the
    /// second. Modifiers act, and stresses, moments and end forces are
    /// reported, in these axes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_x: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "ShellModifiers::is_unmodified")]
    pub modifiers: ShellModifiers,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axes {
    #[default]
    Global,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
    Z,
}
impl Axis {
    pub fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }
}

/// Rigid in-plane constraint. Slave nodes share the master's two in-plane
/// translations and its rotation about the normal, offset by their lever arm.
/// Out-of-plane translation and in-plane rotations of slaves stay independent.
/// Master DOFs with no stiffness are dropped rather than reported unstable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diaphragm {
    pub master: NodeId,
    pub nodes: Vec<NodeId>,
    pub normal: Axis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodalLoad {
    pub node: NodeId,
    #[serde(default)]
    pub force: [Force; 3],
    #[serde(default)]
    pub moment: [Moment; 3],
}
impl NodalLoad {
    pub fn force(node: NodeId, force: [Force; 3]) -> Self {
        Self {
            node,
            force,
            moment: [Moment::ZERO; 3],
        }
    }
    pub fn values(&self) -> [f64; 6] {
        [
            self.force[0].si(),
            self.force[1].si(),
            self.force[2].si(),
            self.moment[0].si(),
            self.moment[1].si(),
            self.moment[2].si(),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberLoad {
    Point {
        member: FrameId,
        position: Length,
        #[serde(default)]
        force: [Force; 3],
        #[serde(default)]
        moment: [Moment; 3],
        #[serde(default)]
        axes: Axes,
    },
    Distributed {
        member: FrameId,
        start: Length,
        end: Length,
        start_load: [LineLoad; 3],
        end_load: [LineLoad; 3],
        #[serde(default)]
        axes: Axes,
    },
}
impl MemberLoad {
    pub fn member(&self) -> FrameId {
        match self {
            Self::Point { member, .. } | Self::Distributed { member, .. } => *member,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceLoad {
    pub shell: ShellId,
    pub pressure: Pressure,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadCase {
    pub name: String,
    /// Self-weight multiplier per global axis, e.g. [0, -1, 0] for gravity in -Y.
    /// Frames get a uniform line load; shells get lumped nodal forces.
    #[serde(default)]
    pub self_weight: [f64; 3],
    #[serde(default)]
    pub nodal: Vec<NodalLoad>,
    #[serde(default)]
    pub member: Vec<MemberLoad>,
    #[serde(default)]
    pub surface: Vec<SurfaceLoad>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadCombination {
    pub name: String,
    pub terms: Vec<(LoadCaseId, f64)>,
}

/// A node whose lateral mass moves, and the nodes it moves to with their shares.
pub type Lump = (NodeId, Vec<(NodeId, f64)>);

/// Where modal and spectrum analysis take mass from, as the mass source of
/// ETABS and SAP2000. Nodal mass always counts. `element_mass` adds the
/// frames' and shells' own mass from material density. Each case in `cases`
/// adds its gravity load, the -Z component, divided by g and scaled by the
/// multiplier: superimposed dead load at 1 and storage live load at 0.25 for
/// ASCE 7 12.7.2, for example. That mass is lumped to nodes as the members'
/// own mass is, a member load statically to its two ends and a surface load
/// by tributary area, and acts in X, Y and Z.
///
/// `lateral` keeps mass in X and Y translation and rotation about Z, and
/// `vertical` keeps Z translation and rotation about X and Y, for every
/// kind of mass alike; at least one must be on. `lump` moves the lateral
/// mass of each listed node onto others in the given shares, which the
/// model layer fills to lump mass between levels onto the levels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MassSource {
    pub element_mass: bool,
    pub cases: Vec<(LoadCaseId, f64)>,
    pub lateral: bool,
    pub vertical: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lump: Vec<Lump>,
}
impl Default for MassSource {
    fn default() -> Self {
        Self {
            element_mass: true,
            cases: vec![],
            lateral: true,
            vertical: true,
            lump: vec![],
        }
    }
}
impl MassSource {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn schema_version() -> u32 {
    1
}
fn standard_gravity() -> Acceleration {
    Acceleration::from_si(STANDARD_GRAVITY)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    /// Used to turn density into self-weight.
    #[serde(default = "standard_gravity")]
    pub gravity: Acceleration,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub materials: Vec<Material>,
    #[serde(default)]
    pub sections: Vec<Section>,
    #[serde(default)]
    pub frames: Vec<Frame>,
    #[serde(default)]
    pub shells: Vec<Shell>,
    #[serde(default)]
    pub diaphragms: Vec<Diaphragm>,
    #[serde(default)]
    pub load_cases: Vec<LoadCase>,
    #[serde(default)]
    pub combinations: Vec<LoadCombination>,
    #[serde(default, skip_serializing_if = "MassSource::is_default")]
    pub mass_source: MassSource,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            schema_version: 1,
            gravity: standard_gravity(),
            nodes: vec![],
            materials: vec![],
            sections: vec![],
            frames: vec![],
            shells: vec![],
            diaphragms: vec![],
            load_cases: vec![],
            combinations: vec![],
            mass_source: MassSource::default(),
        }
    }
}

impl Model {
    pub fn add_node(&mut self, v: Node) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(v);
        id
    }
    pub fn add_material(&mut self, v: Material) -> MaterialId {
        let id = MaterialId(self.materials.len());
        self.materials.push(v);
        id
    }
    pub fn add_section(&mut self, v: Section) -> SectionId {
        let id = SectionId(self.sections.len());
        self.sections.push(v);
        id
    }
    pub fn add_frame(&mut self, v: Frame) -> FrameId {
        let id = FrameId(self.frames.len());
        self.frames.push(v);
        id
    }
    pub fn add_shell(&mut self, v: Shell) -> ShellId {
        let id = ShellId(self.shells.len());
        self.shells.push(v);
        id
    }
    pub fn add_load_case(&mut self, v: LoadCase) -> LoadCaseId {
        let id = LoadCaseId(self.load_cases.len());
        self.load_cases.push(v);
        id
    }
    /// This model with every flexural stiffness modifier below 1 multiplied
    /// by `factor` and capped at 1: `iy` and `iz` on frames, `membrane_x`,
    /// `membrane_y` and `bending` on shells. Unmodified members keep their
    /// gross stiffness. ACI 318 6.6.3.2.2 allows a factor of 1.4 for
    /// deflections under service loads, such as drift under wind.
    pub fn with_cracked_stiffness(&self, factor: f64) -> Model {
        let relax = |m: &mut f64| {
            if *m < 1.0 {
                *m = (*m * factor).min(1.0);
            }
        };
        let mut model = self.clone();
        for f in &mut model.frames {
            relax(&mut f.modifiers.iy);
            relax(&mut f.modifiers.iz);
        }
        for s in &mut model.shells {
            relax(&mut s.modifiers.membrane_x);
            relax(&mut s.modifiers.membrane_y);
            relax(&mut s.modifiers.bending);
        }
        model
    }
    /// Stable content hash: FNV-1a over the canonical JSON. Detects stale
    /// results; not for security.
    pub fn content_hash(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("model serializes");
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    }
    pub fn effective_combinations(&self) -> Vec<LoadCombination> {
        if self.combinations.is_empty() {
            self.load_cases
                .iter()
                .enumerate()
                .map(|(i, c)| LoadCombination {
                    name: c.name.clone(),
                    terms: vec![(LoadCaseId(i), 1.0)],
                })
                .collect()
        } else {
            self.combinations.clone()
        }
    }
    pub fn validate(&self) -> Result<()> {
        let fail = |s: String| Err(Error::Model(s));
        if self.schema_version != 1 {
            return fail(format!(
                "unsupported schema_version {}",
                self.schema_version
            ));
        }
        if self.nodes.is_empty() {
            return fail("no nodes".into());
        }
        if self.frames.is_empty() && self.shells.is_empty() {
            return fail("no elements".into());
        }
        if !self.gravity.si().is_finite() || self.gravity.si() <= 0.0 {
            return fail("gravity must be positive and finite".into());
        }
        for (i, n) in self.nodes.iter().enumerate() {
            let springs = n.springs();
            if springs.iter().any(|k| !k.is_finite() || *k < 0.0) {
                return fail(format!(
                    "node {i}: spring stiffness must be finite and >= 0"
                ));
            }
            if springs.iter().zip(n.restrained).any(|(k, r)| *k > 0.0 && r) {
                return fail(format!(
                    "node {i}: spring on a restrained DOF; remove one or the other"
                ));
            }
            if n.xyz()
                .iter()
                .chain(n.prescribed.values().iter())
                .any(|x| !x.is_finite())
            {
                return fail(format!("node {i}: nonfinite coordinate or settlement"));
            }
            if n.mass
                .iter()
                .map(|v| v.si())
                .chain(n.mass_inertia.iter().map(|v| v.si()))
                .any(|x| !x.is_finite() || x < 0.0)
            {
                return fail(format!("node {i}: invalid mass"));
            }
            if n.prescribed
                .values()
                .iter()
                .zip(n.restrained)
                .any(|(v, r)| *v != 0.0 && !r)
            {
                return fail(format!(
                    "node {i}: prescribed displacement needs a restraint"
                ));
            }
        }
        for (i, m) in self.materials.iter().enumerate() {
            if !m.young.si().is_finite()
                || m.young.si() <= 0.0
                || !m.poisson.is_finite()
                || m.poisson <= -1.0
                || m.poisson >= 0.5
                || !m.density.si().is_finite()
                || m.density.si() < 0.0
            {
                return fail(format!(
                    "material {i}: require E > 0, -1 < nu < 0.5, density >= 0"
                ));
            }
        }
        for (i, s) in self.sections.iter().enumerate() {
            if [s.area.si(), s.iy.si(), s.iz.si(), s.torsion.si()]
                .into_iter()
                .chain([s.shear_y, s.shear_z].into_iter().flatten().map(Area::si))
                .any(|x| !x.is_finite() || x <= 0.0)
            {
                return fail(format!(
                    "section {i}: properties must be positive and finite"
                ));
            }
        }
        for (i, f) in self.frames.iter().enumerate() {
            if f.nodes.iter().any(|n| n.0 >= self.nodes.len())
                || f.material.0 >= self.materials.len()
                || f.section.0 >= self.sections.len()
            {
                return fail(format!("frame {i}: invalid table ID"));
            }
            if !f.roll.si().is_finite()
                || f.local_y.is_some_and(|v| v.iter().any(|x| !x.is_finite()))
            {
                return fail(format!("frame {i}: nonfinite orientation"));
            }
            let l = self.frame_length(FrameId(i));
            if !l.is_finite() || l <= 1e-12 {
                return fail(format!("frame {i}: zero or invalid length"));
            }
            if f.modifiers
                .values()
                .iter()
                .any(|x| !x.is_finite() || *x <= 0.0)
            {
                return fail(format!(
                    "frame {i}: stiffness modifiers must be positive and finite"
                ));
            }
            if [f.modifiers.mass, f.modifiers.weight]
                .iter()
                .any(|x| !x.is_finite() || *x < 0.0)
            {
                return fail(format!(
                    "frame {i}: mass and weight modifiers must be finite and >= 0"
                ));
            }
        }
        for (i, s) in self.shells.iter().enumerate() {
            if s.nodes.iter().any(|n| n.0 >= self.nodes.len())
                || s.material.0 >= self.materials.len()
            {
                return fail(format!("shell {i}: invalid table ID"));
            }
            if !s.thickness.si().is_finite()
                || s.thickness.si() <= 0.0
                || !s.drilling_ratio.is_finite()
                || s.drilling_ratio <= 0.0
                || s.drilling_ratio > 0.01
            {
                return fail(format!(
                    "shell {i}: invalid thickness or drilling_ratio (0, 0.01]"
                ));
            }
            if s.modifiers
                .values()
                .iter()
                .any(|x| !x.is_finite() || *x <= 0.0)
            {
                return fail(format!(
                    "shell {i}: stiffness modifiers must be positive and finite"
                ));
            }
            if [s.modifiers.mass, s.modifiers.weight]
                .iter()
                .any(|x| !x.is_finite() || *x < 0.0)
            {
                return fail(format!(
                    "shell {i}: mass and weight modifiers must be finite and >= 0"
                ));
            }
            if s.local_x.is_some_and(|v| v.iter().any(|x| !x.is_finite())) {
                return fail(format!("shell {i}: nonfinite local_x"));
            }
        }
        let mut slaves = vec![false; self.nodes.len()];
        let mut masters = vec![false; self.nodes.len()];
        for (i, d) in self.diaphragms.iter().enumerate() {
            if d.master.0 >= self.nodes.len()
                || d.nodes.is_empty()
                || d.nodes.iter().any(|n| n.0 >= self.nodes.len())
            {
                return fail(format!("diaphragm {i}: invalid or missing node IDs"));
            }
            if masters[d.master.0] || slaves[d.master.0] {
                return fail(format!(
                    "diaphragm {i}: master node {} already belongs to another diaphragm",
                    d.master.0
                ));
            }
            masters[d.master.0] = true;
            let n = d.normal.index();
            let constrained: Vec<usize> = (0..3).filter(|a| *a != n).chain([3 + n]).collect();
            for s in &d.nodes {
                if s.0 == d.master.0 || slaves[s.0] || masters[s.0] {
                    return fail(format!(
                        "diaphragm {i}: node {} is its master or already constrained",
                        s.0
                    ));
                }
                slaves[s.0] = true;
                let node = &self.nodes[s.0];
                if constrained
                    .iter()
                    .any(|&dof| node.restrained[dof] || node.springs()[dof] > 0.0)
                {
                    return fail(format!(
                        "diaphragm {i}: node {} restrains or springs an in-plane DOF; apply those to the master",
                        s.0
                    ));
                }
            }
        }
        let mut names = std::collections::HashSet::new();
        for (i, c) in self.load_cases.iter().enumerate() {
            if c.name.trim().is_empty() || !names.insert(&c.name) {
                return fail(format!("load case {i}: empty or duplicate name"));
            }
            if c.self_weight.iter().any(|v| !v.is_finite()) {
                return fail(format!("load case {i}: nonfinite self-weight factor"));
            }
            for l in &c.nodal {
                if l.node.0 >= self.nodes.len() || l.values().iter().any(|x| !x.is_finite()) {
                    return fail(format!("load case {i}: invalid nodal load"));
                }
            }
            for l in &c.member {
                if l.member().0 >= self.frames.len() {
                    return fail(format!("load case {i}: invalid member ID"));
                }
                let len = self.frame_length(l.member());
                let valid = match l {
                    MemberLoad::Point {
                        position,
                        force,
                        moment,
                        ..
                    } => {
                        position.si().is_finite()
                            && position.si() >= 0.0
                            && position.si() <= len
                            && force
                                .iter()
                                .map(|v| v.si())
                                .chain(moment.iter().map(|v| v.si()))
                                .all(f64::is_finite)
                    }
                    MemberLoad::Distributed {
                        start,
                        end,
                        start_load,
                        end_load,
                        ..
                    } => {
                        start.si().is_finite()
                            && end.si().is_finite()
                            && start.si() >= 0.0
                            && end.si() <= len
                            && end.si() > start.si()
                            && start_load
                                .iter()
                                .chain(end_load)
                                .all(|v| v.si().is_finite())
                    }
                };
                if !valid {
                    return fail(format!(
                        "load case {i}: invalid load magnitude or position on member {}",
                        l.member().0
                    ));
                }
            }
            for l in &c.surface {
                if l.shell.0 >= self.shells.len() || !l.pressure.si().is_finite() {
                    return fail(format!("load case {i}: invalid surface load"));
                }
            }
        }
        names.clear();
        for c in &self.combinations {
            if c.name.trim().is_empty()
                || !names.insert(&c.name)
                || c.terms.is_empty()
                || c.terms
                    .iter()
                    .any(|(id, f)| id.0 >= self.load_cases.len() || !f.is_finite())
            {
                return fail(format!("invalid combination {:?}", c.name));
            }
        }
        self.validate_mass_source(&self.mass_source)
    }
    /// Checks a mass source against this model: its cases and lumped nodes
    /// exist, its multipliers and shares are valid, and it neither drops
    /// every direction nor counts self-weight twice.
    pub fn validate_mass_source(&self, source: &MassSource) -> Result<()> {
        let fail = |s: String| Err(Error::Model(s));
        if !source.lateral && !source.vertical {
            return fail("mass source: keep lateral or vertical mass".into());
        }
        let mut lumped = vec![false; self.nodes.len()];
        for (from, _) in &source.lump {
            if from.0 >= self.nodes.len() || std::mem::replace(&mut lumped[from.0], true) {
                return fail(format!(
                    "mass source: lumped node {} is missing or listed twice",
                    from.0
                ));
            }
        }
        for (from, targets) in &source.lump {
            let total: f64 = targets.iter().map(|(_, s)| s).sum();
            if targets.is_empty()
                || targets.iter().any(|(t, s)| {
                    t.0 >= self.nodes.len() || lumped[t.0] || !s.is_finite() || *s < 0.0
                })
                || (total - 1.0).abs() > 1e-9
            {
                return fail(format!(
                    "mass source: node {} must lump onto existing nodes that are not lumped themselves, in shares summing to 1",
                    from.0
                ));
            }
        }
        let mut seen = vec![false; self.load_cases.len()];
        for &(id, multiplier) in &source.cases {
            if id.0 >= self.load_cases.len() || std::mem::replace(&mut seen[id.0], true) {
                return fail(format!(
                    "mass source: load case {} is missing or listed twice",
                    id.0
                ));
            }
            if !multiplier.is_finite() || multiplier <= 0.0 {
                return fail(format!(
                    "mass source: multiplier on load case {} must be positive and finite",
                    id.0
                ));
            }
            // Self-weight is the element mass under gravity, so both at once
            // would count it twice.
            if source.element_mass && self.load_cases[id.0].self_weight[2] != 0.0 {
                return fail(format!(
                    "mass source: load case {:?} carries self-weight, which element mass already counts; turn element mass off or use a case without self-weight",
                    self.load_cases[id.0].name
                ));
            }
        }
        Ok(())
    }
    /// Per DOF: None when independent, or the master DOF terms of a slave DOF.
    /// A slave in-plane translation is u_m + theta_n * (n x r); its normal rotation is theta_n.
    pub(crate) fn constraints(&self) -> Vec<Option<Vec<(usize, f64)>>> {
        let mut out = vec![None; self.nodes.len() * 6];
        for d in &self.diaphragms {
            let n = d.normal.index();
            let mut normal = Vector3::zeros();
            normal[n] = 1.0;
            let master = Vector3::from(self.nodes[d.master.0].xyz());
            let rotation = d.master.0 * 6 + 3 + n;
            for s in &d.nodes {
                let arm = normal.cross(&(Vector3::from(self.nodes[s.0].xyz()) - master));
                for a in (0..3).filter(|a| *a != n) {
                    let mut terms = vec![(d.master.0 * 6 + a, 1.0)];
                    if arm[a] != 0.0 {
                        terms.push((rotation, arm[a]));
                    }
                    out[s.0 * 6 + a] = Some(terms);
                }
                out[s.0 * 6 + 3 + n] = Some(vec![(rotation, 1.0)]);
            }
        }
        out
    }
    /// Geometry and orientation checks for one frame, exactly as element
    /// preparation applies them before analysis. Call after [`Model::validate`]
    /// so table indices are known to be in range.
    pub fn validate_frame(&self, index: usize) -> Result<()> {
        crate::element::frame::FrameElement::new(self, index).map(|_| ())
    }
    /// Geometry checks for one shell: degenerate, warped, or non-convex
    /// corners and formulation-specific shape limits.
    pub fn validate_shell(&self, index: usize) -> Result<()> {
        crate::element::shell::ShellElement::new(self, index).map(|_| ())
    }
    pub(crate) fn diaphragm_masters(&self) -> Vec<bool> {
        let mut out = vec![false; self.nodes.len()];
        for d in &self.diaphragms {
            out[d.master.0] = true;
        }
        out
    }
    pub(crate) fn frame_length(&self, id: FrameId) -> f64 {
        let f = &self.frames[id.0];
        let a = self.nodes[f.nodes[0].0].xyz();
        let b = self.nodes[f.nodes[1].0].xyz();
        (0..3).map(|i| (b[i] - a[i]).powi(2)).sum::<f64>().sqrt()
    }
}
