//! Section and material libraries, shipped as data. An entry is copied into
//! the model on use and records where it came from.
use crate::entity::{Material, Provenance, Section};
use oa_core::units::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionEntry {
    pub designation: String,
    /// SI: m^2 and m^4.
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
    /// SI: Pa and kg/m^3.
    pub young: f64,
    pub poisson: f64,
    pub density: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Library {
    pub name: String,
    pub version: String,
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
    /// The small starter library bundled with the crate. Values are common
    /// textbook figures for a handful of shapes; verify against the
    /// governing standard before design use.
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
    /// Copies a section out of the library under the given model name.
    pub fn section(&self, designation: &str, name: impl Into<String>) -> Option<Section> {
        let e = self
            .sections
            .iter()
            .find(|s| s.designation == designation)?;
        Some(Section {
            name: name.into(),
            area: Area::from_si(e.area),
            iy: SecondMoment::from_si(e.iy),
            iz: SecondMoment::from_si(e.iz),
            torsion: SecondMoment::from_si(e.torsion),
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
            young: Pressure::from_si(e.young),
            poisson: e.poisson,
            density: MassDensity::from_si(e.density),
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
