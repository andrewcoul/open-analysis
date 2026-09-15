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
        }
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
                .iter()
                .any(|x| !x.is_finite() || *x <= 0.0)
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
