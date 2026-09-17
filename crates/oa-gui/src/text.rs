//! Number display and parsing at the UI boundary. The model stays SI; every
//! field shows and reads [`UNITS`], converting by the quantity's [`Role`].
use oa_model::{Role, UnitSystem};

/// The units every label, field, and list in the GUI uses.
pub const UNITS: UnitSystem = UnitSystem::UsCustomary;

/// "Name (unit)" for a field label.
pub fn label(name: &str, role: Role) -> String {
    UNITS.label(name, role)
}

/// Short text for an SI value, shown in display units.
pub fn fmt_q(role: Role, si: f64) -> String {
    fmt_num(UNITS.to_display(role, si))
}

/// Parses text typed in display units into an SI value.
pub fn parse_q(role: Role, label: &str, text: &str) -> Result<f64, String> {
    parse_num(label, text).map(|v| UNITS.from_display(role, v))
}

/// Short, round-trippable text for a plain number.
pub fn fmt_num(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-4..1e7).contains(&a) {
        return format!("{v:e}");
    }
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

pub fn parse_num(label: &str, text: &str) -> Result<f64, String> {
    text.trim()
        .replace(',', "")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("{label}: {text:?} is not a number"))
}

#[cfg(test)]
mod tests {
    use super::{fmt_num, fmt_q, label, parse_num, parse_q};
    use oa_model::Role;

    #[test]
    fn formats_compactly() {
        assert_eq!(fmt_num(0.0), "0");
        assert_eq!(fmt_num(3.5), "3.5");
        assert_eq!(fmt_num(2.1e11), "2.1e11");
        assert_eq!(fmt_num(2e-5), "2e-5");
        assert_eq!(fmt_num(-1250.0), "-1250");
    }

    #[test]
    fn parses_scientific_and_rejects_junk() {
        assert_eq!(parse_num("E", " 2.1e11 ").unwrap(), 2.1e11);
        assert_eq!(parse_num("E", "1,000").unwrap(), 1000.0);
        assert!(parse_num("E", "abc").is_err());
        assert!(parse_num("E", "inf").is_err());
    }

    #[test]
    fn fields_show_and_read_us_units() {
        assert_eq!(label("X", Role::Length), "X (ft)");
        assert_eq!(fmt_q(Role::Length, 3.048), "10");
        assert_eq!(fmt_q(Role::Area, 26.5 * 0.0254_f64.powi(2)), "26.5");
        assert!((parse_q(Role::Length, "X", "10").unwrap() - 3.048).abs() < 1e-12);
        assert!((parse_q(Role::Force, "F", "1").unwrap() - 4_448.221_615_260_5).abs() < 1e-9);
        assert!(parse_q(Role::Force, "F", "ten").is_err());
    }
}
