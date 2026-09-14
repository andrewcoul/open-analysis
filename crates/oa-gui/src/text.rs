//! Number display and parsing at the UI boundary. The model stays SI.

/// Short, round-trippable text for an SI value.
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
    use super::*;

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
}
