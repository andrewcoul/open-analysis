//! Plan grids: the rectangular grid ETABS starts a model from, built as
//! ordinary grid lines so each can be edited or removed on its own after.
use crate::command::{Command, ModelError, Result};
use crate::entity::{EntityId, GridLine};
use crate::model::Model;
use oa_core::units::Length;

/// A rectangular grid in plan. X grid lines run along Y and stand at the
/// X spacings from the origin, labelled from `x_label` (A, B, C...); Y grid
/// lines run along X at the Y spacings, labelled from `y_label` (1, 2,
/// 3...). Each line runs `overhang` past the outermost lines it crosses, and
/// its bubble is at the low end.
#[derive(Debug, Clone, PartialEq)]
pub struct RectangularGrid {
    pub origin: [Length; 2],
    /// Distances between successive X grid lines; the first stands at the
    /// origin, so n spacings make n + 1 lines. Empty makes one line.
    pub x_spacings: Vec<Length>,
    pub y_spacings: Vec<Length>,
    pub x_label: String,
    pub y_label: String,
    pub overhang: Length,
}

impl RectangularGrid {
    /// The lines, X grid lines first, each in label order.
    pub fn lines(&self) -> Result<Vec<GridLine>> {
        let positive = |v: &Length| v.si().is_finite() && v.si() > 0.0;
        if !self.x_spacings.iter().chain(&self.y_spacings).all(positive) {
            return Err(ModelError::Invalid(
                "grid spacings must be positive and finite".into(),
            ));
        }
        if !(self.overhang.si().is_finite() && self.overhang.si() >= 0.0) {
            return Err(ModelError::Invalid(
                "grid overhang must be finite and >= 0".into(),
            ));
        }
        let stations = |origin: f64, spacings: &[Length]| {
            std::iter::once(origin)
                .chain(spacings.iter().scan(origin, |at, s| {
                    *at += s.si();
                    Some(*at)
                }))
                .collect::<Vec<f64>>()
        };
        let [ox, oy] = self.origin.map(|v| v.si());
        let xs = stations(ox, &self.x_spacings);
        let ys = stations(oy, &self.y_spacings);
        let over = self.overhang.si();
        // A single line along an axis has nothing to span, so it runs the
        // overhang either side, or 1 m with no overhang, where it would
        // otherwise be a point.
        let span = |at: &[f64]| {
            let (lo, hi) = (at[0], at[at.len() - 1]);
            let pad = if hi > lo || over > 0.0 { over } else { 1.0 };
            (lo - pad, hi + pad)
        };
        let (y0, y1) = span(&ys);
        let (x0, x1) = span(&xs);
        let point = |x: f64, y: f64| [Length::from_si(x), Length::from_si(y)];
        let mut lines = vec![];
        for (x, name) in xs.iter().zip(labels(&self.x_label)?) {
            lines.push(GridLine {
                name,
                start: point(*x, y0),
                end: point(*x, y1),
            });
        }
        for (y, name) in ys.iter().zip(labels(&self.y_label)?) {
            lines.push(GridLine {
                name,
                start: point(x0, *y),
                end: point(x1, *y),
            });
        }
        Ok(lines)
    }

    /// One batch adding every line under fresh ids, so the grid goes in,
    /// and comes out on undo, as one step. A label already in use refuses
    /// the whole batch.
    pub fn command(&self, model: &Model) -> Result<Command> {
        let first = model.next_id;
        let commands = self
            .lines()?
            .into_iter()
            .enumerate()
            .map(|(i, grid_line)| Command::AddGridLine {
                id: EntityId(first.saturating_add(i as u64)),
                grid_line,
            })
            .collect();
        Ok(Command::Batch { commands })
    }
}

/// Labels from `first` on: letters count A to Z then AA, AB..., skipping I
/// and O as drawings do so they are not read as 1 and 0; digits count up;
/// a trailing number after a prefix counts up and keeps the prefix (C1,
/// C2...).
pub fn labels(first: &str) -> Result<impl Iterator<Item = String>> {
    let first = first.trim().to_string();
    if first.is_empty() {
        return Err(ModelError::Invalid("a grid needs a first label".into()));
    }
    let digits = first.len() - first.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let step: Box<dyn Fn(&str) -> String> = if digits > 0 {
        let prefix = first[..first.len() - digits].to_string();
        // Zero padding is kept, so 01 counts on as 02 and 09 as 10.
        let width = digits;
        if first[prefix.len()..].parse::<u32>().is_err() {
            return Err(ModelError::Invalid(format!(
                "grid label {first:?} ends in too large a number"
            )));
        }
        Box::new(move |label: &str| {
            let n: u64 = label[prefix.len()..].parse().unwrap_or(0);
            format!("{prefix}{:0width$}", n + 1)
        })
    } else if first.chars().all(|c| c.is_ascii_alphabetic()) {
        Box::new(next_letters)
    } else {
        return Err(ModelError::Invalid(format!(
            "grid label {first:?} must be letters or end in a number"
        )));
    };
    Ok(std::iter::successors(Some(first), move |l| Some(step(l))))
}

/// The next letter label: B after A, AA after Z, skipping I and O. An
/// all-lowercase label stays lowercase; a mixed-case one counts on in
/// capitals.
fn next_letters(label: &str) -> String {
    let lower = label.chars().all(|c| c.is_ascii_lowercase());
    let mut chars: Vec<u8> = label.to_ascii_uppercase().into_bytes();
    let mut i = chars.len();
    loop {
        if i == 0 {
            chars.insert(0, b'A');
            break;
        }
        i -= 1;
        let next = match chars[i] {
            b'Z' => None,
            b'H' => Some(b'J'),
            b'N' => Some(b'P'),
            c => Some(c + 1),
        };
        match next {
            Some(c) => {
                chars[i] = c;
                break;
            }
            None => chars[i] = b'A',
        }
    }
    let next = String::from_utf8(chars).expect("ASCII");
    if lower {
        next.to_ascii_lowercase()
    } else {
        next
    }
}

/// Spacings typed as a list, "30, 25" or "3@30, 25", each value read by
/// `value`. A count before @ repeats the spacing.
pub fn parse_spacings(
    text: &str,
    mut value: impl FnMut(&str) -> std::result::Result<f64, String>,
) -> std::result::Result<Vec<f64>, String> {
    let mut out = vec![];
    for item in text
        .split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let (count, v) = match item.split_once('@') {
            Some((n, v)) => {
                let n: usize = n
                    .trim()
                    .parse()
                    .map_err(|_| format!("{item:?}: the count before @ must be a whole number"))?;
                (n, v.trim())
            }
            None => (1, item),
        };
        if count == 0 || count > 1000 {
            return Err(format!("{item:?}: repeat 1 to 1000 times"));
        }
        let v = value(v)?;
        out.extend(std::iter::repeat_n(v, count));
    }
    if out.len() > 1000 {
        return Err("a grid holds at most 1000 spacings a direction".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(label: &str, n: usize) -> Vec<String> {
        labels(label).unwrap().take(n).collect()
    }

    #[test]
    fn labels_count_letters_and_numbers() {
        assert_eq!(first("A", 3), ["A", "B", "C"]);
        assert_eq!(first("G", 4), ["G", "H", "J", "K"]);
        assert_eq!(first("M", 3), ["M", "N", "P"]);
        assert_eq!(first("Y", 4), ["Y", "Z", "AA", "AB"]);
        assert_eq!(first("AZ", 2), ["AZ", "BA"]);
        assert_eq!(first("ZZ", 2), ["ZZ", "AAA"]);
        assert_eq!(first("a", 2), ["a", "b"]);
        assert_eq!(first("1", 3), ["1", "2", "3"]);
        assert_eq!(first("9", 2), ["9", "10"]);
        assert_eq!(first("C1", 3), ["C1", "C2", "C3"]);
        assert_eq!(first("A08", 3), ["A08", "A09", "A10"]);
        assert_eq!(first("HZ", 2), ["HZ", "JA"]);
        assert_eq!(first("Ab", 2), ["Ab", "AC"]);
        assert!(labels("C99999999999999999999").is_err());
        assert!(labels("").is_err());
        assert!(labels("A-").is_err());
    }

    #[test]
    fn spacings_repeat_with_at() {
        let value = |s: &str| s.parse::<f64>().map_err(|e| e.to_string());
        assert_eq!(
            parse_spacings("3@30, 25", value).unwrap(),
            [30.0, 30.0, 30.0, 25.0]
        );
        assert_eq!(parse_spacings("", value).unwrap(), Vec::<f64>::new());
        assert!(parse_spacings("0@30", value).is_err());
        assert!(parse_spacings("x@30", value).is_err());
        assert!(parse_spacings("30, y", value).is_err());
    }

    #[test]
    fn a_rectangular_grid_spans_its_lines_with_overhang() {
        let ft = |v: f64| Length::from_si(v);
        let grid = RectangularGrid {
            origin: [ft(1.0), ft(2.0)],
            x_spacings: vec![ft(6.0), ft(4.0)],
            y_spacings: vec![ft(5.0)],
            x_label: "A".into(),
            y_label: "1".into(),
            overhang: ft(0.5),
        };
        let lines = grid.lines().unwrap();
        let names: Vec<&str> = lines.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C", "1", "2"]);
        let si = |l: &GridLine| [l.start, l.end].map(|p| p.map(|v| v.si()));
        assert_eq!(si(&lines[1]), [[7.0, 1.5], [7.0, 7.5]]);
        assert_eq!(si(&lines[4]), [[0.5, 7.0], [11.5, 7.0]]);
        // One line in a direction still has length.
        let single = RectangularGrid {
            y_spacings: vec![],
            overhang: Length::ZERO,
            ..grid.clone()
        };
        let lines = single.lines().unwrap();
        assert_eq!(si(&lines[0]), [[1.0, 1.0], [1.0, 3.0]]);
        let short = RectangularGrid {
            overhang: ft(0.25),
            ..single.clone()
        };
        assert_eq!(si(&short.lines().unwrap()[0]), [[1.0, 1.75], [1.0, 2.25]]);
        let bad = RectangularGrid {
            x_spacings: vec![ft(-1.0)],
            ..grid
        };
        assert!(bad.lines().is_err());
    }
}
