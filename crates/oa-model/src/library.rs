//! Section and material libraries, shipped as data. An entry is copied into
//! the model on use and records where it came from. A library says which
//! units its numbers are in, so tables can be typed in as published.
use crate::entity::{Material, Provenance, Section, SectionProperty, Shape, ShapeKind};
use crate::units::{Role, UnitSystem};
use oa_core::units::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionEntry {
    pub designation: String,
    /// Area and second moments in the library's units; `iz` is the strong axis.
    pub area: f64,
    pub iy: f64,
    pub iz: f64,
    pub torsion: f64,
    /// Design properties in the library's units, for a steel shape.
    #[serde(default)]
    pub shape: Option<Shape>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialEntry {
    pub designation: String,
    /// Modulus and density in the library's units. Density is a weight
    /// density in US customary and a mass density in SI.
    pub young: f64,
    pub poisson: f64,
    pub density: f64,
    /// Specified minimum yield and tensile strengths, for steel, and
    /// compressive strength, for concrete, in the library's stress unit.
    #[serde(default)]
    pub fy: Option<f64>,
    #[serde(default)]
    pub fu: Option<f64>,
    #[serde(default)]
    pub fc: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Library {
    pub name: String,
    pub version: String,
    /// Units the entries are written in; absent means SI.
    #[serde(default)]
    pub units: Option<UnitSystem>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub sections: Vec<SectionEntry>,
    #[serde(default)]
    pub materials: Vec<MaterialEntry>,
}
impl Library {
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
    /// The small starter library bundled with the crate: a few AISC W and
    /// HSS shapes and common US materials, in AISC table units. Verify
    /// against the governing standard before design use.
    pub fn starter() -> Self {
        Self::from_json(include_str!("../data/starter.json")).expect("bundled library parses")
    }
    /// The AISC Shapes Database v16.0, less single and double angles, with
    /// design properties, in AISC table units. It holds sections only.
    pub fn aisc() -> Self {
        Self::from_json(include_str!("../data/aisc_v16.json")).expect("bundled library parses")
    }
    fn provenance(&self, designation: &str) -> Provenance {
        Provenance {
            library: self.name.clone(),
            version: self.version.clone(),
            designation: designation.into(),
        }
    }
    /// An entry's number in SI.
    fn si(&self, role: Role, value: f64) -> f64 {
        match self.units {
            None => value,
            Some(units) => units.from_display(role, value),
        }
    }
    /// The entry with this designation, ignoring case, so "W14x90" finds
    /// AISC's "W14X90".
    pub fn section_entry(&self, designation: &str) -> Option<&SectionEntry> {
        self.sections
            .iter()
            .find(|s| s.designation.eq_ignore_ascii_case(designation))
    }
    /// Copies a section out of the library under the given model name. The
    /// provenance records the library's spelling of the designation.
    pub fn section(&self, designation: &str, name: impl Into<String>) -> Option<Section> {
        let e = self.section_entry(designation)?;
        let shape = e.shape.as_ref().map(|shape| {
            let mut shape = shape.clone();
            for (property, value) in &mut shape.properties {
                if let Some(role) = property.role() {
                    *value = self.si(role, *value);
                }
            }
            shape
        });
        let area = self.si(Role::Area, e.area);
        let [shear_y, shear_z] = shape
            .as_ref()
            .map_or([None, None], |shape| shear_areas(shape, area));
        Some(Section {
            name: name.into(),
            area: Area::from_si(area),
            iy: SecondMoment::from_si(self.si(Role::SecondMoment, e.iy)),
            iz: SecondMoment::from_si(self.si(Role::SecondMoment, e.iz)),
            torsion: SecondMoment::from_si(self.si(Role::SecondMoment, e.torsion)),
            shear_y,
            shear_z,
            shape,
            provenance: Some(self.provenance(&e.designation)),
        })
    }
    pub fn material(&self, designation: &str, name: impl Into<String>) -> Option<Material> {
        let e = self
            .materials
            .iter()
            .find(|m| m.designation == designation)?;
        Some(Material {
            name: name.into(),
            young: Pressure::from_si(self.si(Role::Stress, e.young)),
            poisson: e.poisson,
            density: MassDensity::from_si(self.si(Role::Density, e.density)),
            fy: e.fy.map(|v| Pressure::from_si(self.si(Role::Stress, v))),
            fu: e.fu.map(|v| Pressure::from_si(self.si(Role::Stress, v))),
            fc: e.fc.map(|v| Pressure::from_si(self.si(Role::Stress, v))),
            provenance: Some(self.provenance(designation)),
        })
    }
    pub fn section_designations(&self) -> Vec<&str> {
        self.sections
            .iter()
            .map(|s| s.designation.as_str())
            .collect()
    }
}

/// Shear areas along local y (the web, for AISC Ix bending) and local z,
/// from a shape's SI properties, as ETABS and SAP2000 take them: d tw for
/// a web, 5/6 of each flange's bf tf, 2 t h for the walls of a rectangular
/// tube, and (0.9 - 0.4 s) A for a round one, where s = (r - t) / r is the
/// ratio of inner to outer radius: half the area for a thin wall, rising
/// towards 0.9 A for a solid bar (CSI Analysis Reference, Figure 29). None
/// where a property is missing.
fn shear_areas(shape: &Shape, area: f64) -> [Option<Area>; 2] {
    use SectionProperty::*;
    let p = |property| shape.properties.get(&property).copied();
    let product =
        |a: SectionProperty, b: SectionProperty, factor: f64| Some(factor * p(a)? * p(b)?);
    let web = product(Depth, WebThickness, 1.0);
    let [y, z] = match shape.kind {
        ShapeKind::W
        | ShapeKind::M
        | ShapeKind::S
        | ShapeKind::HP
        | ShapeKind::C
        | ShapeKind::MC => [web, product(FlangeWidth, FlangeThickness, 5.0 / 3.0)],
        ShapeKind::WT | ShapeKind::MT | ShapeKind::ST => {
            [web, product(FlangeWidth, FlangeThickness, 5.0 / 6.0)]
        }
        ShapeKind::HSS if p(OutsideDiameter).is_none() => [
            product(HssDepth, DesignWallThickness, 2.0),
            product(HssWidth, DesignWallThickness, 2.0),
        ],
        ShapeKind::HSS | ShapeKind::PIPE => {
            let round = (|| {
                let r = p(OutsideDiameter)? / 2.0;
                let s = (r - p(DesignWallThickness)?) / r;
                Some((0.9 - 0.4 * s) * area)
            })();
            [round; 2]
        }
    };
    [y, z].map(|a| a.map(Area::from_si))
}
