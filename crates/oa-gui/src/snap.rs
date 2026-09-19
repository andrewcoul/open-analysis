//! Object snaps for the draw tools: the endpoint, midpoint, intersection, or
//! perpendicular foot nearest the pointer, among the frames, shell edges, and
//! underlay lines the view shows. Pure arithmetic, no GPUI. The search runs
//! in screen space, where the pointer is. Intersections and perpendiculars
//! are worked out in plan: a snapped point lands on the active level, so
//! only its X and Y survive.

/// How near the pointer a snap point must be, in pixels.
pub const APERTURE: f64 = 10.0;

/// Segments near the pointer tested pairwise for intersections. Dense line
/// work under the pointer is cut off here rather than squared.
const MAX_NEAR: usize = 32;

/// In priority order: between snap points as near as each other, the earlier wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize)]
pub enum Kind {
    Endpoint,
    Intersection,
    Midpoint,
    Perpendicular,
}

impl Kind {
    /// In the order the status bar and the Draw menu list them.
    pub const ALL: [Kind; 4] = [
        Kind::Endpoint,
        Kind::Midpoint,
        Kind::Intersection,
        Kind::Perpendicular,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Kind::Endpoint => "Endpoint",
            Kind::Intersection => "Intersection",
            Kind::Midpoint => "Midpoint",
            Kind::Perpendicular => "Perpendicular",
        }
    }
}

/// Which snaps are switched on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modes {
    pub endpoint: bool,
    pub midpoint: bool,
    pub intersection: bool,
    pub perpendicular: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Self {
            endpoint: true,
            midpoint: true,
            intersection: true,
            perpendicular: true,
        }
    }
}

impl Modes {
    pub fn has(self, kind: Kind) -> bool {
        match kind {
            Kind::Endpoint => self.endpoint,
            Kind::Midpoint => self.midpoint,
            Kind::Intersection => self.intersection,
            Kind::Perpendicular => self.perpendicular,
        }
    }
    pub fn toggle(&mut self, kind: Kind) {
        let mode = match kind {
            Kind::Endpoint => &mut self.endpoint,
            Kind::Midpoint => &mut self.midpoint,
            Kind::Intersection => &mut self.intersection,
            Kind::Perpendicular => &mut self.perpendicular,
        };
        *mode = !*mode;
    }
    pub fn any(self) -> bool {
        self.endpoint || self.midpoint || self.intersection || self.perpendicular
    }
}

/// A line that can be snapped to, in the model and as painted.
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub world: [[f64; 3]; 2],
    pub screen: [(f64, f64); 2],
}

impl Segment {
    // The projection is affine, so one parameter serves both spaces.
    fn world_at(&self, t: f64) -> [f64; 3] {
        let [a, b] = self.world;
        [0, 1, 2].map(|i| a[i] + t * (b[i] - a[i]))
    }
    fn screen_at(&self, t: f64) -> (f64, f64) {
        let [a, b] = self.screen;
        (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1))
    }
    /// The parameter of the point on the painted segment nearest the pointer.
    fn nearest(&self, pointer: (f64, f64)) -> f64 {
        let [a, b] = self.screen;
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = dx * dx + dy * dy;
        if len2 == 0.0 {
            return 0.0;
        }
        (((pointer.0 - a.0) * dx + (pointer.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub kind: Kind,
    /// The point on the geometry itself, at its own elevation.
    pub world: [f64; 3],
    pub screen: (f64, f64),
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Where in plan two segments cross, as the parameter along each.
fn plan_crossing(p: &Segment, q: &Segment) -> Option<(f64, f64)> {
    let ([a, b], [c, d]) = (p.world, q.world);
    let (r, s) = ([b[0] - a[0], b[1] - a[1]], [d[0] - c[0], d[1] - c[1]]);
    let cross = r[0] * s[1] - r[1] * s[0];
    // Parallel, or one of them vertical and so a point in plan.
    if cross.abs() <= 1e-9 * r[0].hypot(r[1]) * s[0].hypot(s[1]) {
        return None;
    }
    let ac = [c[0] - a[0], c[1] - a[1]];
    let t = (ac[0] * s[1] - ac[1] * s[0]) / cross;
    let u = (ac[0] * r[1] - ac[1] * r[0]) / cross;
    let within = |v: f64| (-1e-9..=1.0 + 1e-9).contains(&v);
    (within(t) && within(u)).then_some((t.clamp(0.0, 1.0), u.clamp(0.0, 1.0)))
}

/// The parameter of the foot of the plan perpendicular from a point, when it
/// falls on the segment and the point is off its line.
fn plan_foot(segment: &Segment, from: [f64; 2]) -> Option<f64> {
    let [a, b] = segment.world;
    let d = [b[0] - a[0], b[1] - a[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    if len2 < 1e-18 {
        return None;
    }
    let t = ((from[0] - a[0]) * d[0] + (from[1] - a[1]) * d[1]) / len2;
    let foot = [a[0] + t * d[0], a[1] + t * d[1]];
    let off_line = (from[0] - foot[0]).hypot(from[1] - foot[1]) > 1e-9;
    ((0.0..=1.0).contains(&t) && off_line).then_some(t)
}

/// The snap point for a pointer position, if one is in reach. `from` is the
/// plan position a perpendicular is dropped from: the last point drawn.
///
/// The nearest snap point within the aperture wins, by priority between
/// equals. Failing that, a pointer resting anywhere on a line takes the foot
/// of the perpendicular to it, however far along the line that is.
pub fn find(
    segments: &[Segment],
    pointer: (f64, f64),
    from: Option<[f64; 2]>,
    modes: Modes,
) -> Option<Hit> {
    if !modes.any() {
        return None;
    }
    let mut near: Vec<(&Segment, f64)> = segments
        .iter()
        .map(|s| (s, distance(pointer, s.screen_at(s.nearest(pointer)))))
        .filter(|(_, d)| *d <= APERTURE)
        .collect();
    near.sort_by(|a, b| a.1.total_cmp(&b.1));
    near.truncate(MAX_NEAR);

    let mut best: Option<(Hit, f64)> = None;
    let mut offer = |kind: Kind, world: [f64; 3], screen: (f64, f64)| {
        let d = distance(pointer, screen);
        if !modes.has(kind) || d > APERTURE {
            return;
        }
        let better = best.is_none_or(|(hit, best_d)| {
            d + 1.0 < best_d || ((d - best_d).abs() <= 1.0 && kind < hit.kind)
        });
        if better {
            best = Some((
                Hit {
                    kind,
                    world,
                    screen,
                },
                d,
            ));
        }
    };
    let foot = |s: &Segment| {
        from.filter(|_| modes.perpendicular)
            .and_then(|p| plan_foot(s, p))
    };
    for (i, (s, _)) in near.iter().enumerate() {
        for t in [0.0, 1.0] {
            offer(Kind::Endpoint, s.world_at(t), s.screen_at(t));
        }
        offer(Kind::Midpoint, s.world_at(0.5), s.screen_at(0.5));
        if let Some(t) = foot(s) {
            offer(Kind::Perpendicular, s.world_at(t), s.screen_at(t));
        }
        for (other, _) in &near[i + 1..] {
            if let Some((t, u)) = plan_crossing(s, other) {
                // Lines on different levels cross in plan at two places on screen.
                offer(Kind::Intersection, s.world_at(t), s.screen_at(t));
                offer(Kind::Intersection, other.world_at(u), other.screen_at(u));
            }
        }
    }
    best.map(|(hit, _)| hit).or_else(|| {
        near.iter().find_map(|(s, _)| {
            let t = foot(s)?;
            Some(Hit {
                kind: Kind::Perpendicular,
                world: s.world_at(t),
                screen: s.screen_at(t),
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{Hit, Kind, Modes, Segment, find};

    /// A plan view at 10 px/m with Y up the screen.
    fn seg(a: [f64; 3], b: [f64; 3]) -> Segment {
        Segment {
            world: [a, b],
            screen: [a, b].map(|p| (p[0] * 10.0, -p[1] * 10.0)),
        }
    }
    fn plan(a: [f64; 2], b: [f64; 2]) -> Segment {
        seg([a[0], a[1], 0.0], [b[0], b[1], 0.0])
    }
    fn only(kind: Kind) -> Modes {
        let mut modes = Modes {
            endpoint: false,
            midpoint: false,
            intersection: false,
            perpendicular: false,
        };
        modes.toggle(kind);
        modes
    }
    fn xy(hit: Option<Hit>) -> Option<(Kind, [f64; 2])> {
        hit.map(|h| (h.kind, [h.world[0], h.world[1]]))
    }

    #[test]
    fn the_nearest_of_an_end_and_the_middle_wins() {
        let line = [plan([0.0, 0.0], [10.0, 0.0])];
        let all = Modes::default();
        assert_eq!(
            xy(find(&line, (3.0, 4.0), None, all)),
            Some((Kind::Endpoint, [0.0, 0.0]))
        );
        assert_eq!(
            xy(find(&line, (97.0, -2.0), None, all)),
            Some((Kind::Endpoint, [10.0, 0.0]))
        );
        assert_eq!(
            xy(find(&line, (52.0, 3.0), None, all)),
            Some((Kind::Midpoint, [5.0, 0.0]))
        );
        // On the line but a long way from either: nothing to take.
        assert_eq!(find(&line, (25.0, 0.0), None, all), None);
        // Off the line altogether.
        assert_eq!(find(&line, (0.0, 40.0), None, all), None);
    }

    #[test]
    fn a_snap_switched_off_is_not_offered() {
        let line = [plan([0.0, 0.0], [10.0, 0.0])];
        assert_eq!(find(&line, (3.0, 4.0), None, only(Kind::Midpoint)), None);
        let mut none = only(Kind::Midpoint);
        none.toggle(Kind::Midpoint);
        assert_eq!(find(&line, (50.0, 0.0), None, none), None);
    }

    #[test]
    fn crossing_lines_snap_where_they_meet() {
        let cross = [
            plan([0.0, 0.0], [10.0, 10.0]),
            plan([0.0, 10.0], [10.0, 0.0]),
        ];
        let hit = find(&cross, (53.0, -48.0), None, Modes::default());
        // The crossing is each line's midpoint too; intersection outranks it.
        assert_eq!(xy(hit), Some((Kind::Intersection, [5.0, 5.0])));
        assert_eq!(hit.unwrap().screen, (50.0, -50.0));
        // Lines that would only meet if extended do not.
        let apart = [plan([0.0, 0.0], [4.0, 4.0]), plan([0.0, 10.0], [10.0, 0.0])];
        assert_eq!(
            find(&apart, (50.0, -50.0), None, only(Kind::Intersection)),
            None
        );
        let parallel = [plan([0.0, 0.0], [10.0, 0.0]), plan([0.0, 0.5], [10.0, 0.5])];
        assert_eq!(
            find(&parallel, (30.0, -2.0), None, only(Kind::Intersection)),
            None
        );
    }

    #[test]
    fn a_shared_corner_reads_as_an_endpoint() {
        let corner = [
            plan([0.0, 0.0], [10.0, 0.0]),
            plan([10.0, 0.0], [10.0, 10.0]),
        ];
        let hit = find(&corner, (98.0, -1.0), None, Modes::default());
        assert_eq!(xy(hit), Some((Kind::Endpoint, [10.0, 0.0])));
    }

    #[test]
    fn a_beam_over_an_underlay_line_crosses_it_in_plan() {
        // The underlay on the ground, the beam a storey up: they never touch,
        // but a node on either level wants the plan crossing.
        let lines = [
            plan([0.0, 5.0], [10.0, 5.0]),
            seg([5.0, 0.0, 3.0], [5.0, 10.0, 3.0]),
        ];
        let hit = find(&lines, (50.0, -50.0), None, only(Kind::Intersection)).unwrap();
        assert_eq!([hit.world[0], hit.world[1]], [5.0, 5.0]);
        // A column is a point in plan and crosses nothing.
        let column = [
            plan([0.0, 5.0], [10.0, 5.0]),
            seg([5.0, 5.0, 0.0], [5.0, 5.0, 3.0]),
        ];
        assert_eq!(
            find(&column, (50.0, -50.0), None, only(Kind::Intersection)),
            None
        );
    }

    #[test]
    fn the_perpendicular_is_dropped_from_the_last_point() {
        let line = [plan([0.0, 0.0], [10.0, 0.0])];
        let from = Some([3.0, 4.0]);
        let all = Modes::default();
        assert_eq!(
            xy(find(&line, (31.0, 2.0), from, all)),
            Some((Kind::Perpendicular, [3.0, 0.0]))
        );
        // Resting anywhere on the line finds the foot, if nothing nearer offers.
        let hit = find(&line, (75.0, 1.0), from, all).unwrap();
        assert_eq!((hit.kind, hit.screen), (Kind::Perpendicular, (30.0, 0.0)));
        assert_eq!(
            xy(find(&line, (98.0, 0.0), from, all)),
            Some((Kind::Endpoint, [10.0, 0.0]))
        );
        // No last point, no perpendicular.
        assert_eq!(find(&line, (31.0, 2.0), None, all), None);
        // The foot falls past the end of the line.
        assert_eq!(find(&line, (75.0, 1.0), Some([14.0, 4.0]), all), None);
        // The last point is on the line itself: every direction along it is
        // perpendicular to nothing.
        assert_eq!(find(&line, (75.0, 1.0), Some([2.0, 0.0]), all), None);
    }

    #[test]
    fn a_sloping_line_gives_the_plan_foot_at_its_own_elevation() {
        let brace = [seg([0.0, 0.0, 0.0], [10.0, 0.0, 5.0])];
        let hit = find(
            &brace,
            (40.0, 0.0),
            Some([4.0, 6.0]),
            only(Kind::Perpendicular),
        )
        .unwrap();
        assert_eq!(hit.world, [4.0, 0.0, 2.0]);
    }
}
