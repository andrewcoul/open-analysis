//! Member results: which section force the view draws along the members, and
//! the shear, moment, and deflection plots on a frame's Results tab. The
//! numbers come from [`oa_core::FrameDiagram`]s the document computes when
//! the analysis runs; this module only chooses and draws them.
use crate::text::{UNITS, fmt_q};
use crate::viewport::{paint_label, stroke_segments};
use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, Theme, h_flex, v_flex};
use gpui_kit::*;
use oa_core::FrameDiagram;
use oa_model::Role;
use serde::Deserialize;

/// A section force the view can draw along every member.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Diagram {
    Axial,
    ShearY,
    ShearZ,
    Torsion,
    MomentY,
    MomentZ,
}

impl Diagram {
    pub const ALL: [Diagram; 6] = [
        Diagram::Axial,
        Diagram::ShearY,
        Diagram::ShearZ,
        Diagram::Torsion,
        Diagram::MomentY,
        Diagram::MomentZ,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Diagram::Axial => "Axial force N",
            Diagram::ShearY => "Shear Vy",
            Diagram::ShearZ => "Shear Vz",
            Diagram::Torsion => "Torsion T",
            Diagram::MomentY => "Moment My",
            Diagram::MomentZ => "Moment Mz",
        }
    }
    pub fn role(self) -> Role {
        match self {
            Diagram::Axial | Diagram::ShearY | Diagram::ShearZ => Role::Force,
            Diagram::Torsion | Diagram::MomentY | Diagram::MomentZ => Role::Moment,
        }
    }
    /// Column of `FrameDiagram::forces`.
    pub fn index(self) -> usize {
        match self {
            Diagram::Axial => 0,
            Diagram::ShearY => 1,
            Diagram::ShearZ => 2,
            Diagram::Torsion => 3,
            Diagram::MomentY => 4,
            Diagram::MomentZ => 5,
        }
    }
    /// Local axis (1 = y, 2 = z) the diagram is drawn along: the plane of
    /// bending for shear and moment, local y for the others.
    pub fn axis(self) -> usize {
        match self {
            Diagram::ShearZ | Diagram::MomentY => 2,
            _ => 1,
        }
    }
}

/// A plane of bending on the Results tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    /// Local x-y: Vy, Mz, and deflection along y.
    XY,
    /// Local x-z: Vz, My, and deflection along z.
    XZ,
}

impl Plane {
    pub const ALL: [Plane; 2] = [Plane::XY, Plane::XZ];
    fn shear(self) -> Diagram {
        match self {
            Plane::XY => Diagram::ShearY,
            Plane::XZ => Diagram::ShearZ,
        }
    }
    fn moment(self) -> Diagram {
        match self {
            Plane::XY => Diagram::MomentZ,
            Plane::XZ => Diagram::MomentY,
        }
    }
    fn deflection(self) -> (usize, &'static str) {
        match self {
            Plane::XY => (0, "Deflection y"),
            Plane::XZ => (1, "Deflection z"),
        }
    }
    /// The plane with the larger moment, so the tab opens on the one that matters.
    pub fn dominant(diagram: &FrameDiagram) -> Plane {
        let peak = |ix: usize| {
            diagram
                .forces
                .iter()
                .map(|f| f[ix].abs())
                .fold(0.0, f64::max)
        };
        if peak(Diagram::MomentY.index()) > peak(Diagram::MomentZ.index()) {
            Plane::XZ
        } else {
            Plane::XY
        }
    }
}

/// Largest magnitude of one column across every member's diagram.
pub fn peak(diagrams: &[FrameDiagram], column: usize) -> f64 {
    diagrams
        .iter()
        .flat_map(|d| d.forces.iter().map(|f| f[column].abs()))
        .fold(0.0, f64::max)
}

/// Stations worth a number: both ends, and the interior extreme when it is
/// larger than either end. Near-zero values are left unlabelled, measured
/// against `scale`, the largest value in the drawing.
pub fn labelled_stations(values: &[f64], scale: f64) -> Vec<usize> {
    let n = values.len();
    if n < 2 {
        return vec![];
    }
    let mut out = vec![0, n - 1];
    let ends = values[0].abs().max(values[n - 1].abs());
    if let Some(k) = (1..n - 1).max_by(|a, b| values[*a].abs().total_cmp(&values[*b].abs()))
        && values[k].abs() > ends * 1.001
    {
        out.insert(1, k);
    }
    out.retain(|k| values[*k].abs() > scale * 5e-3);
    out
}

// MARK: Results tab

/// The Results tab of a frame: its shear, moment, and deflection in one
/// plane of bending, with a switch between the two planes.
pub fn render_member_results(
    diagram: &FrameDiagram,
    combination: &str,
    plane: Plane,
    on_plane: impl Fn(&Plane, &mut Window, &mut App) + 'static,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let style = window.text_style();
    let (shear, moment) = (plane.shear(), plane.moment());
    let (deflection_ix, deflection_label) = plane.deflection();
    let column = |ix: usize| diagram.forces.iter().map(|f| f[ix]).collect::<Vec<f64>>();
    let deflections: Vec<f64> = diagram.deflections.iter().map(|d| d[deflection_ix]).collect();
    let planes = ButtonGroup::new("results-plane")
        .small()
        .outline()
        .child(
            Button::new("plane-xy")
                .label("Plane x-y · Vy, Mz")
                .selected(plane == Plane::XY),
        )
        .child(
            Button::new("plane-xz")
                .label("Plane x-z · Vz, My")
                .selected(plane == Plane::XZ),
        )
        .on_click(move |clicks, window, cx| {
            if let Some(ix) = clicks.first()
                && let Some(plane) = Plane::ALL.get(*ix)
            {
                on_plane(plane, window, cx);
            }
        });
    v_flex()
        .gap_3()
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(format!("Combination {combination}")),
        )
        .child(planes)
        .child(plot(
            shear.label(),
            shear.role(),
            &diagram.stations,
            column(shear.index()),
            theme.warning,
            theme,
            &style,
        ))
        .child(plot(
            moment.label(),
            moment.role(),
            &diagram.stations,
            column(moment.index()),
            theme.warning,
            theme,
            &style,
        ))
        .child(plot(
            deflection_label,
            Role::Displacement,
            &diagram.stations,
            deflections,
            theme.chart_1,
            theme,
            &style,
        ))
        .into_any_element()
}

/// One quantity along the member: a title with its extremes, then the curve
/// filled to the zero line with the ends and the peak labelled.
fn plot(
    title: &str,
    role: Role,
    stations: &[f64],
    values: Vec<f64>,
    color: Hsla,
    theme: &Theme,
    style: &TextStyle,
) -> AnyElement {
    let (muted, fg, border) = (theme.muted_foreground, theme.foreground, theme.border);
    let peak = values.iter().map(|v| v.abs()).fold(0.0, f64::max);
    let extremes = match values
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .zip(values.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1)))
    {
        Some(((kmax, max), (kmin, min))) => format!(
            "max {} at {} ft · min {} at {} ft",
            fmt_q(role, *max),
            fmt_q(Role::Length, stations[kmax]),
            fmt_q(role, *min),
            fmt_q(Role::Length, stations[kmin]),
        ),
        None => String::new(),
    };
    let stations = stations.to_vec();
    let style = style.clone();
    let labelled = labelled_stations(&values, peak);
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .text_xs()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("{title} ({})", UNITS.symbol(role))),
                )
                .child(div().text_color(muted).child(extremes)),
        )
        .child(
            canvas(
                move |_, _, _| (stations, values, labelled),
                move |bounds, (stations, values, labelled), window, cx| {
                    let (left, right) = (bounds.left() + px(8.), bounds.right() - px(8.));
                    let (top, bottom) = (bounds.top() + px(14.), bounds.bottom() - px(14.));
                    let zero = point(left, (top + bottom) / 2.);
                    let length = stations.last().copied().unwrap_or(1.0).max(1e-9);
                    let half = f32::from(bottom - top) / 2.;
                    let scale = if peak > 0.0 { half / peak as f32 } else { 0.0 };
                    let at = |k: usize| {
                        point(
                            left + (right - left) * (stations[k] / length) as f32,
                            zero.y - px(values[k] as f32 * scale),
                        )
                    };
                    let curve: Vec<Point<Pixels>> = (0..values.len()).map(at).collect();
                    let mut outline = vec![point(left, zero.y)];
                    outline.extend(curve.iter().copied());
                    outline.push(point(right, zero.y));
                    let mut builder = PathBuilder::fill();
                    builder.add_polygon(&outline, true);
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, color.opacity(0.2));
                    }
                    stroke_segments(
                        std::iter::once((point(left, zero.y), point(right, zero.y))),
                        px(1.),
                        border,
                        window,
                    );
                    stroke_segments(
                        curve.windows(2).map(|w| (w[0], w[1])),
                        px(1.5),
                        color,
                        window,
                    );
                    for k in labelled {
                        let p = curve[k];
                        let text: SharedString = fmt_q(role, values[k]).into();
                        let above = values[k] >= 0.0;
                        let x = if k == 0 {
                            p.x
                        } else if k == values.len() - 1 {
                            p.x - px(6. * text.len() as f32)
                        } else {
                            p.x - px(3. * text.len() as f32)
                        };
                        let y = if above { p.y - px(15.) } else { p.y + px(2.) };
                        paint_label(&text, point(x, y), fg, &style, window, cx);
                    }
                },
            )
            .w_full()
            .h(px(120.)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::labelled_stations;

    #[test]
    fn labels_ends_and_a_larger_interior_peak() {
        assert_eq!(labelled_stations(&[1.0, 3.0, 1.0], 3.0), vec![0, 1, 2]);
        assert_eq!(labelled_stations(&[4.0, 3.0, -1.0], 4.0), vec![0, 2]);
        assert_eq!(labelled_stations(&[0.0, -2.0, 0.0], 2.0), vec![1]);
        assert_eq!(labelled_stations(&[0.0], 1.0), Vec::<usize>::new());
    }
}
