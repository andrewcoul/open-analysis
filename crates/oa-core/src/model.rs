use crate::{Error, Result, units::*};
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
}
impl Node {
    pub fn new(position: [Length; 3]) -> Self {
        Self {
            position,
            restrained: [false; 6],
            prescribed: Default::default(),
            mass: [Mass::ZERO; 3],
            mass_inertia: [MassInertia::ZERO; 3],
        }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
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
    pub load_cases: Vec<LoadCase>,
    #[serde(default)]
    pub combinations: Vec<LoadCombination>,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            schema_version: 1,
            nodes: vec![],
            materials: vec![],
            sections: vec![],
            frames: vec![],
            shells: vec![],
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
        for (i, n) in self.nodes.iter().enumerate() {
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
        let mut names = std::collections::HashSet::new();
        for (i, c) in self.load_cases.iter().enumerate() {
            if c.name.trim().is_empty() || !names.insert(&c.name) {
                return fail(format!("load case {i}: empty or duplicate name"));
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
    pub(crate) fn frame_length(&self, id: FrameId) -> f64 {
        let f = &self.frames[id.0];
        let a = self.nodes[f.nodes[0].0].xyz();
        let b = self.nodes[f.nodes[1].0].xyz();
        (0..3).map(|i| (b[i] - a[i]).powi(2)).sum::<f64>().sqrt()
    }
}
