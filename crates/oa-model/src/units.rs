//! Display units. The solver, the model, and the saved file are SI; this is
//! the one place that knows what a person or an agent sees and types. Every
//! boundary (the GUI, the MCP server, the library data) converts through
//! [`UnitSystem`], so a second system is another table of factors here and
//! nothing else.
//!
//! Quantities are keyed by [`Role`] rather than by dimension, because the
//! usual US practice mixes units within a dimension: coordinates in feet
//! but section properties and shell thickness in inches, forces in kips but
//! stresses in ksi. The one system built so far is kip-ft-in.
use crate::command::Command;
use crate::entity::*;
use oa_core::units::*;
use serde::{Deserialize, Serialize};

/// What a number means at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Node coordinates, load positions along a member, spans, heights.
    Length,
    /// Shell thickness.
    Thickness,
    /// Node displacement, as a result or a prescribed value.
    Displacement,
    /// Node rotation, as a result or a prescribed value.
    Rotation,
    /// A frame's roll about its axis.
    Angle,
    Area,
    /// Second moment of area and torsion constant.
    SecondMoment,
    Force,
    Moment,
    /// Force per length along a member, and shell membrane and shear
    /// results.
    LineLoad,
    /// Moment per length: shell bending results.
    MomentPerLength,
    /// Surface pressure on a shell.
    Pressure,
    /// Stress and elastic modulus.
    Stress,
    Mass,
    MassInertia,
    /// Translational spring.
    Stiffness,
    /// Rotational spring.
    RotationalStiffness,
    /// Material density. Shown as a weight density; the model stores mass
    /// density, and the two differ by standard gravity.
    Density,
    Acceleration,
}
impl Role {
    pub const ALL: [Role; 19] = [
        Role::Length,
        Role::Thickness,
        Role::Displacement,
        Role::Rotation,
        Role::Angle,
        Role::Area,
        Role::SecondMoment,
        Role::Force,
        Role::Moment,
        Role::LineLoad,
        Role::MomentPerLength,
        Role::Pressure,
        Role::Stress,
        Role::Mass,
        Role::MassInertia,
        Role::Stiffness,
        Role::RotationalStiffness,
        Role::Density,
        Role::Acceleration,
    ];
}

/// One display unit: its symbol and how many SI units it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    pub symbol: &'static str,
    /// SI value of one display unit.
    pub si: f64,
}

const INCH: f64 = 0.0254;
const FOOT: f64 = 0.3048;
const POUND_FORCE: f64 = 4.448_221_615_260_5;
const POUND_MASS: f64 = 0.453_592_37;
const KIP: f64 = 1000.0 * POUND_FORCE;

/// A set of display units. US customary is the only one built; SI is the
/// next table to add, selected by `Metadata::display_units`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitSystem {
    /// Kips, feet, and inches as US structural practice mixes them.
    #[default]
    UsCustomary,
}
impl UnitSystem {
    pub fn name(self) -> &'static str {
        match self {
            UnitSystem::UsCustomary => "US customary (kip, ft, in)",
        }
    }
    pub fn unit(self, role: Role) -> Unit {
        let (symbol, si) = match self {
            UnitSystem::UsCustomary => match role {
                Role::Length => ("ft", FOOT),
                Role::Thickness => ("in", INCH),
                Role::Displacement => ("in", INCH),
                Role::Rotation => ("rad", 1.0),
                Role::Angle => ("deg", std::f64::consts::PI / 180.0),
                Role::Area => ("in²", INCH * INCH),
                Role::SecondMoment => ("in⁴", INCH.powi(4)),
                Role::Force => ("kip", KIP),
                Role::Moment => ("kip·ft", KIP * FOOT),
                Role::LineLoad => ("kip/ft", KIP / FOOT),
                Role::MomentPerLength => ("kip·ft/ft", KIP),
                Role::Pressure => ("psf", POUND_FORCE / (FOOT * FOOT)),
                Role::Stress => ("ksi", KIP / (INCH * INCH)),
                Role::Mass => ("kip·s²/ft", KIP / FOOT),
                Role::MassInertia => ("kip·s²·ft", KIP * FOOT),
                Role::Stiffness => ("kip/ft", KIP / FOOT),
                Role::RotationalStiffness => ("kip·ft/rad", KIP * FOOT),
                Role::Density => ("pcf", POUND_MASS / FOOT.powi(3)),
                Role::Acceleration => ("ft/s²", FOOT),
            },
        };
        Unit { symbol, si }
    }
    pub fn symbol(self, role: Role) -> &'static str {
        self.unit(role).symbol
    }
    /// `label` with the unit symbol appended, as "X (ft)".
    pub fn label(self, label: &str, role: Role) -> String {
        format!("{label} ({})", self.symbol(role))
    }
    pub fn to_display(self, role: Role, si: f64) -> f64 {
        si / self.unit(role).si
    }
    pub fn from_display(self, role: Role, value: f64) -> f64 {
        value * self.unit(role).si
    }
    /// A copy of `value` with every quantity in display units.
    pub fn display<T: MapQuantities + Clone>(self, value: &T) -> T {
        let mut out = value.clone();
        out.map_quantities(&mut |role, si| self.to_display(role, si));
        out
    }
    /// A copy of `value`, given in display units, converted to SI.
    pub fn si<T: MapQuantities + Clone>(self, value: &T) -> T {
        let mut out = value.clone();
        out.map_quantities(&mut |role, v| self.from_display(role, v));
        out
    }
    /// Every role's symbol, for an agent to read once.
    pub fn symbols(self) -> Vec<(Role, &'static str)> {
        Role::ALL.iter().map(|r| (*r, self.symbol(*r))).collect()
    }
}

/// Rescales every quantity in a value and leaves names, ids, flags, and
/// dimensionless factors alone. The closure sees the role and the current
/// number and returns the new one.
pub trait MapQuantities {
    fn map_quantities(&mut self, f: &mut dyn FnMut(Role, f64) -> f64);
}

macro_rules! remap {
    ($f:ident, $role:expr, $field:expr, $t:ty) => {
        $field = <$t>::from_si($f($role, ($field).si()));
    };
}

impl MapQuantities for Node {
    fn map_quantities(&mut self, f: &mut dyn FnMut(Role, f64) -> f64) {
        for i in 0..3 {
            remap!(f, Role::Length, self.position[i], Length);
            remap!(
                f,
                Role::Displacement,
                self.prescribed.translation[i],
                Length
            );
            remap!(f, Role::Rotation, self.prescribed.rotation[i], Angle);
            remap!(f, Role::Mass, self.mass[i], Mass);
            remap!(f, Role::MassInertia, self.mass_inertia[i], MassInertia);
            remap!(f, Role::Stiffness, self.spring_translation[i], Stiffness);
            remap!(
                f,
                Role::RotationalStiffness,
                self.spring_rotation[i],
                RotationalStiffness
            );
        }
    }
}
impl MapQuantities for Material {
    fn map_quantities(&mut self, f: &mut dyn FnMut(Role, f64) -> f64) {
        remap!(f, Role::Stress, self.young, Pressure);
        remap!(f, Role::Density, self.density, MassDensity);
    }
}
impl MapQuantities for Section {
    fn map_quantities(&mut self, f: &mut dyn FnMut(Role, f64) -> f64) {
        remap!(f, Role::Area, self.area, Area);
        remap!(f, Role::SecondMoment, self.iy, SecondMoment);
        remap!(f, Role::SecondMoment, self.iz, SecondMoment);
        remap!(f, Role::SecondMoment, self.torsion, SecondMoment);
    }
}
impl MapQuantities for Frame {
    fn map_quantities(&mut self, f: &mut dyn FnMut(Role, f64) -> f64) {
        remap!(f, Role::Angle, self.roll, Angle);
    }
}
impl MapQuantities for Shell {
    fn map_quantities(&mut self, f: &mut dyn FnMut(Role, f64) -> f64) {
        remap!(f, Role::Thickness, self.thickness, Length);
    }
}
impl MapQuantities for Diaphragm {
    fn map_quantities(&mut self, _: &mut dyn FnMut(Role, f64) -> f64) {}
}
impl MapQuantities for Combination {
    fn map_quantities(&mut self, _: &mut dyn FnMut(Role, f64) -> f64) {}
}
impl MapQuantities for Group {
    fn map_quantities(&mut self, _: &mut dyn FnMut(Role, f64) -> f64) {}
}
impl MapQuantities for NodalLoad {
    fn map_quantities(&mut self, f: &mut dyn FnMut(Role, f64) -> f64) {
        for i in 0..3 {
            remap!(f, Role::Force, self.force[i], Force);
            remap!(f, Role::Moment, self.moment[i], Moment);
        }
    }
}
impl MapQuantities for MemberLoad {
    fn map_quantities(&mut self, f: &mut dyn FnMut(Role, f64) -> f64) {
        match self {
            MemberLoad::Point {
                position,
                force,
                moment,
                ..
            } => {
                remap!(f, Role::Length, *position, Length);
                for i in 0..3 {
                    remap!(f, Role::Force, force[i], Force);
                    remap!(f, Role::Moment, moment[i], Moment);
                }
            }
            MemberLoad::Distributed {
                start,
                end,
                start_load,
                end_load,
                ..
            } => {
                remap!(f, Role::Length, *start, Length);
                remap!(f, Role::Length, *end, Length);
                for i in 0..3 {
                    remap!(f, Role::LineLoad, start_load[i], LineLoad);
                    remap!(f, Role::LineLoad, end_load[i], LineLoad);
                }
            }
        }
    }
}
impl MapQuantities for SurfaceLoad {
    fn map_quantities(&mut self, f: &mut dyn FnMut(Role, f64) -> f64) {
        remap!(f, Role::Pressure, self.pressure, Pressure);
    }
}
impl MapQuantities for LoadCase {
    fn map_quantities(&mut self, f: &mut dyn FnMut(Role, f64) -> f64) {
        // Self weight is a vector of gravity multiples and stays as it is.
        for l in &mut self.nodal {
            l.map_quantities(f);
        }
        for l in &mut self.member {
            l.map_quantities(f);
        }
        for l in &mut self.surface {
            l.map_quantities(f);
        }
    }
}
impl MapQuantities for Command {
    fn map_quantities(&mut self, f: &mut dyn FnMut(Role, f64) -> f64) {
        match self {
            Command::AddNode { node, .. } | Command::UpdateNode { node, .. } => {
                node.map_quantities(f)
            }
            Command::AddMaterial { material, .. } | Command::UpdateMaterial { material, .. } => {
                material.map_quantities(f)
            }
            Command::AddSection { section, .. } | Command::UpdateSection { section, .. } => {
                section.map_quantities(f)
            }
            Command::AddFrame { frame, .. } | Command::UpdateFrame { frame, .. } => {
                frame.map_quantities(f)
            }
            Command::AddShell { shell, .. } | Command::UpdateShell { shell, .. } => {
                shell.map_quantities(f)
            }
            Command::AddLoadCase { load_case, .. } | Command::UpdateLoadCase { load_case, .. } => {
                load_case.map_quantities(f)
            }
            Command::SetGravity { gravity } => {
                remap!(f, Role::Acceleration, *gravity, Acceleration);
            }
            Command::Batch { commands } => {
                for c in commands {
                    c.map_quantities(f);
                }
            }
            Command::AddDiaphragm { .. }
            | Command::UpdateDiaphragm { .. }
            | Command::AddCombination { .. }
            | Command::UpdateCombination { .. }
            | Command::AddGroup { .. }
            | Command::UpdateGroup { .. }
            | Command::SetMetadata { .. }
            | Command::RemoveNode { .. }
            | Command::RemoveMaterial { .. }
            | Command::RemoveSection { .. }
            | Command::RemoveFrame { .. }
            | Command::RemoveShell { .. }
            | Command::RemoveDiaphragm { .. }
            | Command::RemoveLoadCase { .. }
            | Command::RemoveCombination { .. }
            | Command::RemoveGroup { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntityId;

    const US: UnitSystem = UnitSystem::UsCustomary;

    /// Within the precision the expected figures are quoted to.
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-4 * b.abs().max(1e-12)
    }

    #[test]
    fn familiar_values_convert_as_engineers_expect() {
        // 29 000 ksi is 200 GPa steel; 490 pcf steel is 7850 kg/m³.
        assert!(close(US.from_display(Role::Stress, 29_000.0), 1.999_47e11));
        assert!(
            close(US.to_display(Role::Density, 7850.0), 490.06),
            "{}",
            US.to_display(Role::Density, 7850.0)
        );
        assert!(close(US.from_display(Role::Length, 10.0), 3.048));
        assert!(close(
            US.from_display(Role::Force, 1.0),
            4_448.221_615_260_5
        ));
        assert!(close(
            US.from_display(Role::Moment, 1.0),
            1_355.817_948_331_4
        ));
        assert!(close(
            US.from_display(Role::Pressure, 100.0),
            4_788.025_898_033_6
        ));
        assert!(close(
            US.from_display(Role::Acceleration, 32.174_05),
            9.806_65
        ));
        assert!(close(
            US.from_display(Role::Angle, 180.0),
            std::f64::consts::PI
        ));
        // One kip·s²/ft is a thousand slugs.
        assert!(
            close(US.from_display(Role::Mass, 1.0), 14_593.9),
            "{}",
            US.from_display(Role::Mass, 1.0)
        );
    }

    #[test]
    fn every_role_round_trips() {
        for role in Role::ALL {
            let v = 123.456;
            let back = US.to_display(role, US.from_display(role, v));
            assert!((back - v).abs() <= 1e-12 * v, "{role:?}: {back}");
            assert!(!US.symbol(role).is_empty());
        }
    }

    #[test]
    fn entities_and_commands_map_every_quantity_and_nothing_else() {
        let mut node = Node::fixed("N", [Length::from_feet(10.0); 3]);
        node.mass = [Mass::from_si(14_593.9); 3];
        node.prescribed.rotation = [Angle::from_si(0.5); 3];
        let shown = US.display(&node);
        assert!(close(shown.position[0].si(), 10.0));
        assert!(close(shown.mass[1].si(), 1.0));
        assert!(close(shown.prescribed.rotation[2].si(), 0.5));
        assert_eq!(shown.restrained, [true; 6]);
        assert_eq!(US.si(&shown), node);

        let mut case = LoadCase::new("D");
        case.self_weight = [0.0, 0.0, -1.0];
        case.member.push(MemberLoad::Distributed {
            member: EntityId(4),
            start: Length::ZERO,
            end: Length::from_feet(20.0),
            start_load: [
                LineLoad::ZERO,
                LineLoad::ZERO,
                LineLoad::from_si(-KIP / FOOT),
            ],
            end_load: [
                LineLoad::ZERO,
                LineLoad::ZERO,
                LineLoad::from_si(-KIP / FOOT),
            ],
            axes: Axes::Global,
        });
        case.surface.push(SurfaceLoad {
            shell: EntityId(5),
            pressure: Pressure::from_si(US.from_display(Role::Pressure, 50.0)),
        });
        let command = Command::Batch {
            commands: vec![
                Command::AddLoadCase {
                    id: EntityId(8),
                    load_case: case.clone(),
                },
                Command::SetGravity {
                    gravity: Acceleration::from_si(STANDARD_GRAVITY),
                },
            ],
        };
        let shown = US.display(&command);
        let Command::Batch { commands } = &shown else {
            panic!()
        };
        let Command::AddLoadCase { load_case, .. } = &commands[0] else {
            panic!()
        };
        assert_eq!(load_case.self_weight, [0.0, 0.0, -1.0]);
        let MemberLoad::Distributed { end, end_load, .. } = &load_case.member[0] else {
            panic!()
        };
        assert!(close(end.si(), 20.0));
        assert!(close(end_load[2].si(), -1.0));
        assert!(close(load_case.surface[0].pressure.si(), 50.0));
        let Command::SetGravity { gravity } = &commands[1] else {
            panic!()
        };
        assert!(close(gravity.si(), 32.174_05), "{}", gravity.si());
        assert_eq!(US.si(&shown), command);
    }
}
