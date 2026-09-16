//! Section and material libraries, shipped as data. An entry is copied into
//! the model on use and records where it came from. A library says which
//! units its numbers are in, so tables can be typed in as published.
use crate::entity::{Material, Provenance, Section};
use crate::units::{Role, UnitSystem};
use oa_core::units::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionEntry {
    pub designation: String,
    /// Area and second moments in the library's units; `iz` is the strong axis.
    pub area: f64,
    pub iy: f64,
    pub iz: f64,
    pub torsion: f64,
    /// Properties the solver does not need yet, such as plastic moduli, kept for design.
    #[serde(default)]
    pub extra: BTreeMap<String, f64>,
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
    /// Copies a section out of the library under the given model name.
    pub fn section(&self, designation: &str, name: impl Into<String>) -> Option<Section> {
        let e = self
            .sections
            .iter()
            .find(|s| s.designation == designation)?;
        Some(Section {
            name: name.into(),
            area: Area::from_si(self.si(Role::Area, e.area)),
            iy: SecondMoment::from_si(self.si(Role::SecondMoment, e.iy)),
            iz: SecondMoment::from_si(self.si(Role::SecondMoment, e.iz)),
            torsion: SecondMoment::from_si(self.si(Role::SecondMoment, e.torsion)),
            provenance: Some(self.provenance(designation)),
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
