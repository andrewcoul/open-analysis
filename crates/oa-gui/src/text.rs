//! Number display and parsing at the UI boundary. The model stays SI; every
//! field shows and reads [`UNITS`], converting by the quantity's [`Role`].
use oa_model::{Role, UnitSystem};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The units every label, field, and list in the GUI uses.
pub const UNITS: UnitSystem = UnitSystem::UsCustomary;

/// Decimals the GUI shows until Edit > Precision says otherwise.
pub const DEFAULT_PRECISION: usize = 2;

/// The choices Edit > Precision offers.
pub const PRECISIONS: [usize; 7] = [0, 1, 2, 3, 4, 5, 6];

/// Decimals every number is shown with. It is a global because numbers are
/// formatted deep inside field, label, and table builders that carry no
/// window; only the model layer is affected, and it keeps full precision.
static PRECISION: AtomicUsize = AtomicUsize::new(DEFAULT_PRECISION);

/// Decimals numbers are shown with.
pub fn precision() -> usize {
    PRECISION.load(Ordering::Relaxed)
}

/// Shows every number with this many decimals from here on. Views already
/// built keep their text until they are rebuilt.
pub fn set_precision(decimals: usize) {
    PRECISION.store(decimals, Ordering::Relaxed);
}

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

/// A plain number at the current [`precision`]. Magnitudes too small or too
/// large to read as fixed text stay in scientific notation.
pub fn fmt_num(v: f64) -> String {
    let p = precision();
    if v == 0.0 {
        return format!("{:.p$}", 0.0);
    }
    let a = v.abs();
    if !(1e-4..1e7).contains(&a) {
        return format!("{v:e}");
    }
    format!("{v:.p$}")
}

pub fn parse_num(label: &str, text: &str) -> Result<f64, String> {
    text.trim()
        .replace(',', "")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("{label}: {text:?} is not a number"))
}

/// The precision is process-wide, so tests that depend on it take turns
/// through this guard, which puts the default back when it drops.
#[cfg(test)]
pub struct TestPrecision {
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl TestPrecision {
    pub fn of(decimals: usize) -> Self {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_precision(decimals);
        Self { _guard: guard }
    }
}

#[cfg(test)]
impl Drop for TestPrecision {
    fn drop(&mut self) {
        set_precision(DEFAULT_PRECISION);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_PRECISION, TestPrecision, fmt_num, fmt_q, label, parse_num, parse_q, set_precision,
    };
    use oa_model::Role;

    #[test]
    fn formats_with_two_decimals_by_default() {
        let _p = TestPrecision::of(DEFAULT_PRECISION);
        assert_eq!(fmt_num(0.0), "0.00");
        assert_eq!(fmt_num(-0.0), "0.00");
        assert_eq!(fmt_num(3.5), "3.50");
        assert_eq!(fmt_num(-1250.0), "-1250.00");
        assert_eq!(fmt_num(2.1e11), "2.1e11");
        assert_eq!(fmt_num(2e-5), "2e-5");
    }

    #[test]
    fn follows_the_chosen_precision() {
        let _p = TestPrecision::of(0);
        assert_eq!(fmt_num(1.23456), "1");
        assert_eq!(fmt_num(0.0), "0");
        set_precision(4);
        assert_eq!(fmt_num(1.23456), "1.2346");
        assert_eq!(fmt_num(2.1e11), "2.1e11");
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
        let _p = TestPrecision::of(DEFAULT_PRECISION);
        assert_eq!(label("X", Role::Length), "X (ft)");
        assert_eq!(fmt_q(Role::Length, 3.048), "10.00");
        assert_eq!(fmt_q(Role::Area, 26.5 * 0.0254_f64.powi(2)), "26.50");
        assert!((parse_q(Role::Length, "X", "10").unwrap() - 3.048).abs() < 1e-12);
        assert!((parse_q(Role::Force, "F", "1").unwrap() - 4_448.221_615_260_5).abs() < 1e-9);
        assert!(parse_q(Role::Force, "F", "ten").is_err());
    }
}
