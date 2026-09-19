//! Reads a DXF drawing as plan line work for an underlay. Model-space lines,
//! polylines, arcs, circles, and ellipses are kept, curves as short chords,
//! and block inserts are expanded in place. Z is dropped: an underlay lies
//! flat on its level. Text, hatches, splines, meshes, and dimensions are skipped,
//! and so is anything the drawing hides: invisible entities and layers that are
//! switched off. Frozen layers are not told apart; the `dxf` crate does not read
//! that flag.
use dxf::entities::{Entity, EntityType};
use std::collections::{HashMap, HashSet};
use std::f64::consts::TAU;
use std::path::Path;

/// Drawing units the import dialog offers, with their length in metres.
pub const UNITS: [(&str, f64); 5] = [
    ("Inches", 0.0254),
    ("Feet", 0.3048),
    ("Millimetres", 0.001),
    ("Centimetres", 0.01),
    ("Metres", 1.0),
];

pub type Segment = [[f64; 2]; 2];

pub struct Drawing {
    /// Segment ends in plan, in drawing units.
    pub segments: Vec<Segment>,
    /// The entry of [`UNITS`] the file declares, when it declares one of them.
    pub unit: Option<usize>,
    /// Entities of a kind that is not read.
    pub skipped: usize,
}

pub fn read(path: &Path) -> Result<Drawing, String> {
    let drawing = dxf::Drawing::load_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    flatten(&drawing).map_err(|e| format!("{}: {e}", path.display()))
}

fn flatten(drawing: &dxf::Drawing) -> Result<Drawing, String> {
    let unit = match drawing.header.default_drawing_units {
        dxf::enums::Units::Inches => Some(0),
        dxf::enums::Units::Feet => Some(1),
        dxf::enums::Units::Millimeters => Some(2),
        dxf::enums::Units::Centimeters => Some(3),
        dxf::enums::Units::Meters => Some(4),
        _ => None,
    };
    let blocks = drawing.blocks().map(|b| (b.name.as_str(), b)).collect();
    let off = drawing
        .layers()
        .filter(|l| !l.is_layer_on)
        .map(|l| l.name.to_lowercase())
        .collect();
    let mut reader = Reader {
        blocks,
        off,
        open: vec![],
        segments: vec![],
        skipped: 0,
        work: 0,
    };
    for entity in drawing.entities() {
        reader.entity(entity, IDENTITY, "0");
    }
    if reader.work > BUDGET {
        return Err(format!(
            "the drawing expands to more than {BUDGET} entities and segments"
        ));
    }
    if reader.segments.is_empty() {
        return Err("no line work found".into());
    }
    Ok(Drawing {
        segments: reader.segments,
        unit,
        skipped: reader.skipped,
    })
}

/// A plan affine map: x' = m[0] x + m[1] y + m[2], y' = m[3] x + m[4] y + m[5].
type Transform = [f64; 6];
const IDENTITY: Transform = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

fn apply(m: &Transform, p: [f64; 2]) -> [f64; 2] {
    [
        m[0] * p[0] + m[1] * p[1] + m[2],
        m[3] * p[0] + m[4] * p[1] + m[5],
    ]
}
/// The map that applies `inner` and then `outer`.
fn compose(outer: &Transform, inner: &Transform) -> Transform {
    [
        outer[0] * inner[0] + outer[1] * inner[3],
        outer[0] * inner[1] + outer[1] * inner[4],
        outer[0] * inner[2] + outer[1] * inner[5] + outer[2],
        outer[3] * inner[0] + outer[4] * inner[3],
        outer[3] * inner[1] + outer[4] * inner[4],
        outer[3] * inner[2] + outer[4] * inner[5] + outer[5],
    ]
}
/// Planar entities store their points in a frame set by their normal. For a
/// normal along +Z that frame is the world's; along -Z, as mirroring leaves
/// it, its X axis is reversed. Drawings tilted out of plan are not handled.
fn object_frame(outer: &Transform, normal: &dxf::Vector) -> Transform {
    if normal.z < 0.0 {
        compose(outer, &[-1.0, 0.0, 0.0, 0.0, 1.0, 0.0])
    } else {
        *outer
    }
}

/// Inserts nested deeper than this are dropped, to bound the recursion.
const MAX_DEPTH: usize = 16;
/// Entities visited plus segments made before a drawing is refused. Nested
/// blocks multiply: a few kilobytes of inserts can expand without end, and the
/// read runs on the interface thread.
const BUDGET: usize = 2_000_000;
/// Chords per full turn of an arc.
const CHORDS: f64 = 48.0;

struct Reader<'a> {
    blocks: HashMap<&'a str, &'a dxf::Block>,
    /// Layers that are switched off, in lower case: layer names match without case.
    off: HashSet<String>,
    /// The blocks being expanded, outermost first.
    open: Vec<&'a str>,
    segments: Vec<Segment>,
    skipped: usize,
    /// Entities visited plus segments made, against [`BUDGET`].
    work: usize,
}

impl<'a> Reader<'a> {
    /// `layer0` is the layer that contents drawn on layer 0 take: inside a
    /// block, the layer of its insert.
    fn entity(&mut self, entity: &'a Entity, m: Transform, layer0: &'a str) {
        self.work += 1;
        let layer = match entity.common.layer.as_str() {
            "0" => layer0,
            own => own,
        };
        if self.work > BUDGET
            || entity.common.is_in_paper_space
            || !entity.common.is_visible
            || self.off.contains(&layer.to_lowercase())
        {
            return;
        }
        match &entity.specific {
            EntityType::Line(line) => {
                self.push([
                    apply(&m, [line.p1.x, line.p1.y]),
                    apply(&m, [line.p2.x, line.p2.y]),
                ]);
            }
            EntityType::Circle(circle) => {
                let m = object_frame(&m, &circle.normal);
                let centre = [circle.center.x, circle.center.y];
                self.arc(&m, centre, circle.radius, 0.0, TAU);
            }
            EntityType::Arc(arc) => {
                let m = object_frame(&m, &arc.normal);
                let start = arc.start_angle.to_radians();
                let sweep = (arc.end_angle.to_radians() - start).rem_euclid(TAU);
                let sweep = if sweep == 0.0 { TAU } else { sweep };
                self.arc(&m, [arc.center.x, arc.center.y], arc.radius, start, sweep);
            }
            EntityType::Ellipse(ellipse) => {
                // Centre and axes are world coordinates; the normal only
                // says which way round the parameter runs.
                let major = [ellipse.major_axis.x, ellipse.major_axis.y];
                let side = ellipse.minor_axis_ratio * ellipse.normal.z.signum();
                let minor = [-major[1] * side, major[0] * side];
                let start = ellipse.start_parameter;
                let sweep = (ellipse.end_parameter - start).rem_euclid(TAU);
                let sweep = if sweep == 0.0 { TAU } else { sweep };
                let centre = [ellipse.center.x, ellipse.center.y];
                self.curve(&m, sweep, |t| {
                    let (sin, cos) = (start + t).sin_cos();
                    [
                        centre[0] + major[0] * cos + minor[0] * sin,
                        centre[1] + major[1] * cos + minor[1] * sin,
                    ]
                });
            }
            EntityType::LwPolyline(poly) => {
                let m = object_frame(&m, &poly.extrusion_direction);
                let vertices: Vec<([f64; 2], f64)> = poly
                    .vertices
                    .iter()
                    .map(|v| ([v.x, v.y], v.bulge))
                    .collect();
                self.polyline(&m, &vertices, poly.is_closed());
            }
            // A mesh stores faces among its vertices, not a run to join up.
            EntityType::Polyline(poly) if poly.is_polyface_mesh() || poly.is_3d_polygon_mesh() => {
                self.skipped += 1;
            }
            EntityType::Polyline(poly) => {
                let m = object_frame(&m, &poly.normal);
                let vertices: Vec<([f64; 2], f64)> = poly
                    .vertices()
                    .map(|v| ([v.location.x, v.location.y], v.bulge))
                    .collect();
                self.polyline(&m, &vertices, poly.is_closed());
            }
            EntityType::Insert(insert) => {
                let Some(block) = self.blocks.get(insert.name.as_str()).copied() else {
                    self.skipped += 1;
                    return;
                };
                // A block that reaches itself again would never finish.
                if self.open.len() >= MAX_DEPTH || self.open.contains(&block.name.as_str()) {
                    self.skipped += 1;
                    return;
                }
                let (sin, cos) = insert.rotation.to_radians().sin_cos();
                let (sx, sy) = (insert.x_scale_factor, insert.y_scale_factor);
                let (bx, by) = (block.base_point.x, block.base_point.y);
                let (a, b, c, d) = (cos * sx, -sin * sy, sin * sx, cos * sy);
                let place = [
                    a,
                    b,
                    insert.location.x - a * bx - b * by,
                    c,
                    d,
                    insert.location.y - c * bx - d * by,
                ];
                let m = compose(&object_frame(&m, &insert.extrusion_direction), &place);
                self.open.push(block.name.as_str());
                for entity in &block.entities {
                    if self.work > BUDGET {
                        break;
                    }
                    self.entity(entity, m, layer);
                }
                self.open.pop();
            }
            _ => self.skipped += 1,
        }
    }

    fn push(&mut self, segment: Segment) {
        self.work += 1;
        if self.work <= BUDGET {
            self.segments.push(segment);
        }
    }
    /// Chords along `point(t)` for t from zero to `sweep` radians.
    fn curve(&mut self, m: &Transform, sweep: f64, point: impl Fn(f64) -> [f64; 2]) {
        let chords = (sweep.abs() / TAU * CHORDS).ceil().max(1.0) as usize;
        let mut last = apply(m, point(0.0));
        for i in 1..=chords {
            let next = apply(m, point(sweep * i as f64 / chords as f64));
            self.push([last, next]);
            last = next;
        }
    }
    fn arc(&mut self, m: &Transform, centre: [f64; 2], radius: f64, start: f64, sweep: f64) {
        self.curve(m, sweep, |t| {
            let (sin, cos) = (start + t).sin_cos();
            [centre[0] + radius * cos, centre[1] + radius * sin]
        });
    }
    /// Vertices with the bulge of the span that leaves each: the tangent of a
    /// quarter of the arc's angle, positive when it runs anticlockwise.
    fn polyline(&mut self, m: &Transform, vertices: &[([f64; 2], f64)], closed: bool) {
        let spans = match (vertices.len(), closed) {
            (0 | 1, _) => 0,
            (n, true) => n,
            (n, false) => n - 1,
        };
        for i in 0..spans {
            let (p, bulge) = vertices[i];
            let (q, _) = vertices[(i + 1) % vertices.len()];
            let chord = (q[0] - p[0]).hypot(q[1] - p[1]);
            if bulge == 0.0 || chord == 0.0 {
                self.push([apply(m, p), apply(m, q)]);
                continue;
            }
            // The centre sits on the chord's left normal, this far along it.
            let along = chord * (1.0 - bulge * bulge) / (4.0 * bulge);
            let centre = [
                (p[0] + q[0]) / 2.0 - (q[1] - p[1]) / chord * along,
                (p[1] + q[1]) / 2.0 + (q[0] - p[0]) / chord * along,
            ];
            let radius = (p[0] - centre[0]).hypot(p[1] - centre[1]);
            let start = (p[1] - centre[1]).atan2(p[0] - centre[0]);
            self.arc(m, centre, radius, start, 4.0 * bulge.atan());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Segment, flatten};
    use dxf::entities::{Arc, Entity, EntityType, Insert, Line, LwPolyline, Polyline, Vertex};
    use dxf::tables::Layer;
    use dxf::{Block, LwPolylineVertex, Point};

    fn line(a: [f64; 2], b: [f64; 2]) -> Entity {
        Entity::new(EntityType::Line(Line::new(
            Point::new(a[0], a[1], 5.0),
            Point::new(b[0], b[1], 5.0),
        )))
    }
    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-9
    }
    /// The chords run end to end from `from` to `to`, every joint `radius` from `centre`.
    fn on_arc(segments: &[Segment], from: [f64; 2], to: [f64; 2], centre: [f64; 2], radius: f64) {
        assert!(close(segments[0][0], from), "{:?}", segments[0]);
        assert!(close(segments[segments.len() - 1][1], to));
        for pair in segments.windows(2) {
            assert!(close(pair[0][1], pair[1][0]));
        }
        for s in segments {
            let r = (s[1][0] - centre[0]).hypot(s[1][1] - centre[1]);
            assert!((r - radius).abs() < 1e-9);
        }
    }

    #[test]
    fn lines_drop_z_and_report_the_declared_unit() {
        let mut drawing = dxf::Drawing::new();
        assert!(flatten(&drawing).is_err(), "nothing to read");
        drawing.header.default_drawing_units = dxf::enums::Units::Millimeters;
        drawing.add_entity(line([0.0, 0.0], [6000.0, 0.0]));
        drawing.add_entity(Entity::new(EntityType::Text(Default::default())));
        let read = flatten(&drawing).unwrap();
        assert_eq!(read.segments, vec![[[0.0, 0.0], [6000.0, 0.0]]]);
        assert_eq!(read.unit, Some(2));
        assert_eq!(read.skipped, 1);
    }

    #[test]
    fn arcs_and_bulges_become_chords_on_the_curve() {
        let mut drawing = dxf::Drawing::new();
        let arc = Arc::new(Point::new(1.0, 1.0, 0.0), 2.0, 270.0, 90.0);
        drawing.add_entity(Entity::new(EntityType::Arc(arc)));
        let read = flatten(&drawing).unwrap();
        // Anticlockwise through the wrap at 360 degrees: half a turn.
        assert_eq!(read.segments.len(), 24);
        on_arc(&read.segments, [1.0, -1.0], [1.0, 3.0], [1.0, 1.0], 2.0);
        assert!(read.segments.iter().all(|s| s[1][0] >= 1.0 - 1e-9));

        // A bulge of one is a semicircle, anticlockwise from the first vertex:
        // heading +X it swings through -Y. Closing adds the straight return.
        let mut drawing = dxf::Drawing::new();
        let mut poly = LwPolyline {
            vertices: vec![
                LwPolylineVertex {
                    bulge: 1.0,
                    ..Default::default()
                },
                LwPolylineVertex {
                    x: 4.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        poly.set_is_closed(true);
        drawing.add_entity(Entity::new(EntityType::LwPolyline(poly)));
        let read = flatten(&drawing).unwrap();
        let (curve, back) = read.segments.split_at(read.segments.len() - 1);
        on_arc(curve, [0.0, 0.0], [4.0, 0.0], [2.0, 0.0], 2.0);
        assert!(curve.iter().all(|s| s[1][1] <= 1e-9));
        assert_eq!(back, [[[4.0, 0.0], [0.0, 0.0]]]);
    }

    #[test]
    fn inserts_place_scale_and_turn_their_block() {
        let mut drawing = dxf::Drawing::new();
        drawing.add_block(Block {
            name: "column".into(),
            base_point: Point::new(1.0, 0.0, 0.0),
            entities: vec![line([1.0, 0.0], [2.0, 0.0])],
            ..Default::default()
        });
        let insert = |name: &str| Insert {
            name: name.into(),
            location: Point::new(10.0, 20.0, 0.0),
            x_scale_factor: 3.0,
            rotation: 90.0,
            ..Default::default()
        };
        drawing.add_entity(Entity::new(EntityType::Insert(insert("column"))));
        drawing.add_entity(Entity::new(EntityType::Insert(insert("missing"))));
        let read = flatten(&drawing).unwrap();
        assert_eq!(read.segments.len(), 1);
        assert!(close(read.segments[0][0], [10.0, 20.0]));
        assert!(close(read.segments[0][1], [10.0, 23.0]));
        assert_eq!(read.skipped, 1);

        // A block that reaches itself is expanded once, however many times
        // it does so: each branch would otherwise double the work per level.
        let mut drawing = dxf::Drawing::new();
        let again = || {
            Entity::new(EntityType::Insert(Insert {
                name: "loop".into(),
                ..Default::default()
            }))
        };
        drawing.add_block(Block {
            name: "loop".into(),
            entities: vec![line([0.0, 0.0], [1.0, 0.0]), again(), again()],
            ..Default::default()
        });
        drawing.add_entity(again());
        let read = flatten(&drawing).unwrap();
        assert_eq!(read.segments.len(), 1);
        assert_eq!(read.skipped, 2);
    }

    /// Eight inserts a block, fifteen blocks deep, is 8^15 lines from a few
    /// kilobytes. The read gives up instead of filling memory.
    #[test]
    fn nesting_that_multiplies_without_end_is_refused() {
        let mut drawing = dxf::Drawing::new();
        for i in 0..15 {
            let inner = Insert {
                name: format!("b{}", i + 1),
                ..Default::default()
            };
            drawing.add_block(Block {
                name: format!("b{i}"),
                entities: vec![Entity::new(EntityType::Insert(inner)); 8],
                ..Default::default()
            });
        }
        drawing.add_block(Block {
            name: "b15".into(),
            entities: vec![line([0.0, 0.0], [1.0, 0.0])],
            ..Default::default()
        });
        drawing.add_entity(Entity::new(EntityType::Insert(Insert {
            name: "b0".into(),
            ..Default::default()
        })));
        assert!(flatten(&drawing).is_err_and(|e| e.contains("expands")));
    }

    #[test]
    fn what_the_drawing_hides_stays_hidden() {
        let on_layer = |mut entity: Entity, layer: &str| {
            entity.common.layer = layer.into();
            entity
        };
        let mut drawing = dxf::Drawing::new();
        drawing.add_layer(Layer {
            name: "Alternates".into(),
            is_layer_on: false,
            ..Default::default()
        });
        drawing.add_entity(line([0.0, 0.0], [1.0, 0.0]));
        let mut invisible = line([0.0, 1.0], [1.0, 1.0]);
        invisible.common.is_visible = false;
        drawing.add_entity(invisible);
        // Layer names match without regard to case.
        drawing.add_entity(on_layer(line([0.0, 2.0], [1.0, 2.0]), "ALTERNATES"));
        // Block contents on layer 0 take the layer of their insert; contents
        // on a layer of their own keep it.
        drawing.add_block(Block {
            name: "mark".into(),
            entities: vec![
                line([0.0, 3.0], [1.0, 3.0]),
                on_layer(line([0.0, 4.0], [1.0, 4.0]), "Alternates"),
            ],
            ..Default::default()
        });
        let mark = || {
            Entity::new(EntityType::Insert(Insert {
                name: "mark".into(),
                ..Default::default()
            }))
        };
        drawing.add_entity(mark());
        drawing.add_entity(on_layer(mark(), "Alternates"));
        let read = flatten(&drawing).unwrap();
        assert_eq!(
            read.segments,
            vec![[[0.0, 0.0], [1.0, 0.0]], [[0.0, 3.0], [1.0, 3.0]]]
        );
        assert_eq!(read.skipped, 0, "hidden is not the same as unreadable");
    }

    /// A polyface mesh lists its faces after its corners, as vertices at the
    /// origin. Joined up like a polyline they would draw a line to nowhere.
    #[test]
    fn meshes_are_skipped_not_joined_up() {
        let mut drawing = dxf::Drawing::new();
        let mut mesh = Polyline::default();
        mesh.set_is_polyface_mesh(true);
        for (x, y) in [(10.0, 10.0), (20.0, 10.0), (20.0, 20.0), (10.0, 20.0)] {
            mesh.add_vertex(&mut drawing, Vertex::new(Point::new(x, y, 0.0)));
        }
        mesh.add_vertex(&mut drawing, Vertex::default());
        drawing.add_entity(Entity::new(EntityType::Polyline(mesh)));
        drawing.add_entity(line([0.0, 0.0], [1.0, 0.0]));
        let read = flatten(&drawing).unwrap();
        assert_eq!(read.segments, vec![[[0.0, 0.0], [1.0, 0.0]]]);
        assert_eq!(read.skipped, 1);
    }
}
