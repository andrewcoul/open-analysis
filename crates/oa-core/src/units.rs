//! Quantity newtypes store SI values. Serialization always uses SI numbers.
//! No implicit conversions between dimensions or between weight and mass.
use serde::{Deserialize, Serialize};

/// Standard gravity in metres per second squared.
pub const STANDARD_GRAVITY: f64 = 9.80665;

macro_rules! quantity {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Default, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(f64);
        impl $name {
            pub const ZERO: Self = Self(0.0);
            pub const fn from_si(value: f64) -> Self {
                Self(value)
            }
            pub const fn si(self) -> f64 {
                self.0
            }
        }
    };
}
quantity!(Length, "Length in metres.");
quantity!(Area, "Area in square metres.");
quantity!(
    SecondMoment,
    "Second moment of area / torsion constant in metres to the fourth power."
);
quantity!(Force, "Force in newtons.");
quantity!(Moment, "Moment in newton metres.");
quantity!(Pressure, "Stress / pressure / elastic modulus in pascals.");
quantity!(LineLoad, "Force per length in newtons per metre.");
quantity!(Mass, "Mass in kilograms.");
quantity!(
    MassInertia,
    "Rotational mass moment in kilogram metres squared."
);
quantity!(
    Stiffness,
    "Translational spring stiffness in newtons per metre."
);
quantity!(
    RotationalStiffness,
    "Rotational spring stiffness in newton metres per radian."
);
quantity!(MassDensity, "Mass density in kilograms per cubic metre.");
quantity!(WeightDensity, "Weight density in newtons per cubic metre.");
quantity!(Acceleration, "Acceleration in metres per second squared.");
quantity!(Angle, "Angle in radians.");

impl Length {
    pub const fn from_metres(v: f64) -> Self {
        Self(v)
    }
    pub fn from_mm(v: f64) -> Self {
        Self(v * 0.001)
    }
    pub fn from_inches(v: f64) -> Self {
        Self(v * 0.0254)
    }
    pub fn from_feet(v: f64) -> Self {
        Self(v * 0.3048)
    }
    pub fn feet(self) -> f64 {
        self.0 / 0.3048
    }
}
impl Area {
    pub fn from_square_inches(v: f64) -> Self {
        Self(v * 0.0254_f64.powi(2))
    }
}
impl SecondMoment {
    pub fn from_inches4(v: f64) -> Self {
        Self(v * 0.0254_f64.powi(4))
    }
}
impl Force {
    pub fn from_kn(v: f64) -> Self {
        Self(v * 1000.0)
    }
    pub fn from_lbf(v: f64) -> Self {
        Self(v * 4.448_221_615_260_5)
    }
    pub fn from_kips(v: f64) -> Self {
        Self::from_lbf(v * 1000.0)
    }
}
impl Pressure {
    pub fn from_mpa(v: f64) -> Self {
        Self(v * 1e6)
    }
    pub fn from_gpa(v: f64) -> Self {
        Self(v * 1e9)
    }
    pub fn from_psi(v: f64) -> Self {
        Self(v * 4.448_221_615_260_5 / 0.0254_f64.powi(2))
    }
    pub fn from_ksi(v: f64) -> Self {
        Self::from_psi(v * 1000.0)
    }
}
impl MassDensity {
    pub fn from_lbm_per_ft3(v: f64) -> Self {
        Self(v * 0.453_592_37 / 0.3048_f64.powi(3))
    }
}
impl WeightDensity {
    pub fn from_lbf_per_ft3(v: f64) -> Self {
        Self(v * 4.448_221_615_260_5 / 0.3048_f64.powi(3))
    }
    pub fn to_mass_density(self, gravity: Acceleration) -> crate::Result<MassDensity> {
        if !gravity.si().is_finite() || gravity.si() <= 0.0 || !self.0.is_finite() || self.0 < 0.0 {
            return Err(crate::Error::Model(
                "weight density must be nonnegative and gravity positive and finite".into(),
            ));
        }
        Ok(MassDensity(self.0 / gravity.si()))
    }
}
impl Angle {
    pub fn from_degrees(v: f64) -> Self {
        Self(v.to_radians())
    }
}
