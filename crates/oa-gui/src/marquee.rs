//! Box selection: a left-drag with the Select tool draws a rectangle, and
//! its direction decides what it takes, as in ETABS and AutoCAD. Dragged
//! left to right it is a window and takes only what lies wholly inside;
//! right to left it is a crossing and takes anything it touches. Pure
//! arithmetic in screen space, no GPUI, so the rules are unit-tested here.

/// What the rectangle takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Dragged left to right: wholly inside only.
    Window,
    /// Dragged right to left: anything the box touches.
    Crossing,
}

/// A rectangle dragged from `start` to `end`, in screen pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marquee {
    pub start: (f64, f64),
    pub end: (f64, f64),
}

impl Marquee {
    pub fn mode(&self) -> Mode {
        if self.end.0 >= self.start.0 {
            Mode::Window
        } else {
            Mode::Crossing
        }
    }
    /// Left, top, right, bottom.
    pub fn rect(&self) -> (f64, f64, f64, f64) {
        (
            self.start.0.min(self.end.0),
            self.start.1.min(self.end.1),
            self.start.0.max(self.end.0),
            self.start.1.max(self.end.1),
        )
    }
    fn contains(&self, p: (f64, f64)) -> bool {
        let (l, t, r, b) = self.rect();
        (l..=r).contains(&p.0) && (t..=b).contains(&p.1)
    }
    /// A node: its point is inside, in either mode.
    pub fn takes_point(&self, p: (f64, f64)) -> bool {
        self.contains(p)
    }
    /// A frame: both ends inside for a window, any part for a crossing.
    pub fn takes_segment(&self, a: (f64, f64), b: (f64, f64)) -> bool {
        match self.mode() {
            Mode::Window => self.contains(a) && self.contains(b),
            Mode::Crossing => self.crosses_segment(a, b),
        }
    }
    /// A shell: every corner inside for a window; for a crossing, any edge
    /// touching the box, or the box lying wholly inside the shell.
    pub fn takes_polygon(&self, poly: &[(f64, f64)]) -> bool {
        match self.mode() {
            Mode::Window => poly.iter().all(|p| self.contains(*p)),
            Mode::Crossing => {
                let n = poly.len();
                (0..n).any(|i| self.crosses_segment(poly[i], poly[(i + 1) % n]))
                    || super::viewport::point_in_polygon(self.start, poly)
            }
        }
    }
    /// Whether any part of the segment lies in the box: Liang-Barsky clipping.
    fn crosses_segment(&self, a: (f64, f64), b: (f64, f64)) -> bool {
        let (l, t, r, bottom) = self.rect();
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for (p, q) in [
            (-dx, a.0 - l),
            (dx, r - a.0),
            (-dy, a.1 - t),
            (dy, bottom - a.1),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let u = q / p;
                if p < 0.0 {
                    t0 = t0.max(u);
                } else {
                    t1 = t1.min(u);
                }
                if t0 > t1 {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Marquee {
        Marquee {
            start: (10.0, 10.0),
            end: (50.0, 40.0),
        }
    }
    fn crossing() -> Marquee {
        Marquee {
            start: (50.0, 40.0),
            end: (10.0, 10.0),
        }
    }

    #[test]
    fn direction_picks_the_mode() {
        assert_eq!(window().mode(), Mode::Window);
        assert_eq!(crossing().mode(), Mode::Crossing);
        // Dragging up or down does not matter, only left or right.
        let up = Marquee {
            start: (10.0, 40.0),
            end: (50.0, 10.0),
        };
        assert_eq!(up.mode(), Mode::Window);
        assert_eq!(up.rect(), window().rect());
    }

    #[test]
    fn nodes_are_taken_when_inside_in_either_mode() {
        for m in [window(), crossing()] {
            assert!(m.takes_point((30.0, 20.0)));
            assert!(m.takes_point((10.0, 40.0)), "the edge counts as inside");
            assert!(!m.takes_point((60.0, 20.0)));
        }
    }

    #[test]
    fn a_window_takes_only_frames_wholly_inside() {
        let m = window();
        assert!(m.takes_segment((20.0, 20.0), (40.0, 30.0)));
        assert!(!m.takes_segment((20.0, 20.0), (80.0, 30.0)), "one end out");
        assert!(
            !m.takes_segment((0.0, 25.0), (80.0, 25.0)),
            "passes through"
        );
    }

    #[test]
    fn a_crossing_takes_frames_it_touches() {
        let m = crossing();
        assert!(m.takes_segment((20.0, 20.0), (40.0, 30.0)), "wholly inside");
        assert!(m.takes_segment((20.0, 20.0), (80.0, 30.0)), "one end in");
        assert!(m.takes_segment((0.0, 25.0), (80.0, 25.0)), "passes through");
        assert!(
            m.takes_segment((30.0, 0.0), (30.0, 60.0)),
            "vertical through"
        );
        assert!(!m.takes_segment((0.0, 0.0), (80.0, 5.0)), "passes above");
        assert!(!m.takes_segment((60.0, 0.0), (60.0, 60.0)), "beside it");
        // The diagonal misses the corner of the box.
        assert!(!m.takes_segment((0.0, 50.0), (60.0, 110.0)));
    }

    #[test]
    fn shells_follow_the_same_rules() {
        let inside = [(20.0, 20.0), (40.0, 20.0), (40.0, 30.0), (20.0, 30.0)];
        let straddling = [(20.0, 20.0), (80.0, 20.0), (80.0, 30.0), (20.0, 30.0)];
        let around = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
        let apart = [(60.0, 0.0), (90.0, 0.0), (90.0, 30.0), (60.0, 30.0)];
        assert!(window().takes_polygon(&inside));
        assert!(!window().takes_polygon(&straddling));
        assert!(!window().takes_polygon(&around));
        assert!(crossing().takes_polygon(&inside));
        assert!(crossing().takes_polygon(&straddling));
        assert!(
            crossing().takes_polygon(&around),
            "a box drawn inside a slab touches the slab"
        );
        assert!(!crossing().takes_polygon(&apart));
    }
}
