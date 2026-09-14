//! The 3D model view: an orthographic wireframe of nodes, frames, and
//! shells painted on a canvas, with orbit, pan, zoom, and click selection.
use crate::camera::{Camera, UpAxis, ViewPreset};
use crate::document::Document;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use oa_model::EntityId;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DisplayOptions {
    pub node_labels: bool,
    pub frame_labels: bool,
    pub deformed: bool,
}

/// Screen positions of the last painted frame, used for picking.
#[derive(Default)]
struct Snapshot {
    bounds: Bounds<Pixels>,
    nodes: Vec<(EntityId, Point<Pixels>)>,
    frames: Vec<(EntityId, Point<Pixels>, Point<Pixels>)>,
    shells: Vec<(EntityId, [Point<Pixels>; 4])>,
}

struct Drag {
    button: MouseButton,
    last: Point<Pixels>,
    moved: bool,
    shift: bool,
}

pub struct Viewport {
    document: Entity<Document>,
    camera: Camera,
    options: DisplayOptions,
    focus_handle: FocusHandle,
    snapshot: Rc<RefCell<Snapshot>>,
    drag: Option<Drag>,
    fit_on_next_paint: bool,
    _subscriptions: Vec<Subscription>,
}

impl Viewport {
    pub fn new(document: Entity<Document>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&document, |_, _, cx| cx.notify());
        Self {
            document,
            camera: Camera::default(),
            options: DisplayOptions::default(),
            focus_handle: cx.focus_handle(),
            snapshot: Rc::new(RefCell::new(Snapshot::default())),
            drag: None,
            fit_on_next_paint: true,
            _subscriptions: vec![subscription],
        }
    }
    pub fn options(&self) -> DisplayOptions {
        self.options
    }
    pub fn up_axis(&self) -> UpAxis {
        self.camera.up
    }
    pub fn set_options(&mut self, options: DisplayOptions, cx: &mut Context<Self>) {
        self.options = options;
        cx.notify();
    }
    pub fn set_preset(&mut self, preset: ViewPreset, cx: &mut Context<Self>) {
        self.camera.set_preset(preset);
        self.fit_on_next_paint = true;
        cx.notify();
    }
    pub fn toggle_up_axis(&mut self, cx: &mut Context<Self>) {
        self.camera.up = match self.camera.up {
            UpAxis::Y => UpAxis::Z,
            UpAxis::Z => UpAxis::Y,
        };
        self.fit_on_next_paint = true;
        cx.notify();
    }
    pub fn zoom_extents(&mut self, cx: &mut Context<Self>) {
        self.fit_on_next_paint = true;
        cx.notify();
    }

    // MARK: Pointer input

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.drag = Some(Drag {
            button: event.button,
            last: event.position,
            moved: false,
            shift: event.modifiers.shift,
        });
    }
    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        let dx = f64::from(event.position.x - drag.last.x);
        let dy = f64::from(event.position.y - drag.last.y);
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        drag.last = event.position;
        if dx.abs() + dy.abs() > 2.0 {
            drag.moved = true;
        }
        match drag.button {
            MouseButton::Right if !drag.shift => self.camera.orbit(dx, dy),
            MouseButton::Right | MouseButton::Middle => self.camera.pan(dx, dy),
            _ => return,
        }
        cx.notify();
    }
    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if drag.button == MouseButton::Left && !drag.moved {
            self.pick(event.position, event.modifiers.shift, cx);
        }
    }
    fn on_mouse_up_out(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag = None;
    }
    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lines = match event.delta {
            ScrollDelta::Lines(p) => f64::from(p.y),
            ScrollDelta::Pixels(p) => f64::from(p.y) / 40.0,
        };
        if lines == 0.0 {
            return;
        }
        let centre = self.snapshot.borrow().bounds.center();
        let offset = (
            f64::from(event.position.x - centre.x),
            f64::from(event.position.y - centre.y),
        );
        self.camera.zoom(1.15f64.powf(lines), offset);
        cx.notify();
    }

    /// Selects the entity under the pointer: nodes first, then frames, then shells.
    fn pick(&mut self, position: Point<Pixels>, extend: bool, cx: &mut Context<Self>) {
        let hit = {
            let snapshot = self.snapshot.borrow();
            let (px, py) = (f64::from(position.x), f64::from(position.y));
            let node = snapshot
                .nodes
                .iter()
                .map(|(id, p)| (*id, distance((px, py), to_f64(*p))))
                .filter(|(_, d)| *d <= 8.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(id, _)| id);
            node.or_else(|| {
                snapshot
                    .frames
                    .iter()
                    .map(|(id, a, b)| (*id, segment_distance((px, py), to_f64(*a), to_f64(*b))))
                    .filter(|(_, d)| *d <= 6.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(id, _)| id)
            })
            .or_else(|| {
                snapshot
                    .shells
                    .iter()
                    .rev()
                    .find(|(_, pts)| point_in_polygon((px, py), &pts.map(to_f64)))
                    .map(|(id, _)| *id)
            })
        };
        self.document
            .update(cx, |document, cx| match (hit, extend) {
                (Some(id), true) => document.toggle_selected(id, cx),
                (Some(id), false) => document.set_selection(vec![id], cx),
                (None, false) => document.clear_selection(cx),
                (None, true) => {}
            });
    }
}

fn to_f64(p: Point<Pixels>) -> (f64, f64) {
    (f64::from(p.x), f64::from(p.y))
}
fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}
fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    distance(p, (a.0 + t * dx, a.1 + t * dy))
}
fn point_in_polygon(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > p.1) != (yj > p.1) && p.0 < (xj - xi) * (p.1 - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

// MARK: Painting

struct Palette {
    frame: Hsla,
    shell: Hsla,
    node: Hsla,
    restraint: Hsla,
    selected: Hsla,
    deformed: Hsla,
    label: Hsla,
    axes: [Hsla; 3],
}

struct NodeMark {
    position: Point<Pixels>,
    restrained: bool,
    selected: bool,
    label: Option<SharedString>,
}
struct Segment {
    a: Point<Pixels>,
    b: Point<Pixels>,
    selected: bool,
    label: Option<SharedString>,
}
struct Quad {
    points: [Point<Pixels>; 4],
    depth: f64,
    selected: bool,
}

/// Everything the paint closure needs, computed once per frame in prepaint.
struct Scene {
    bounds: Bounds<Pixels>,
    nodes: Vec<NodeMark>,
    frames: Vec<Segment>,
    shells: Vec<Quad>,
    deformed_frames: Vec<(Point<Pixels>, Point<Pixels>)>,
    axes: [(Point<Pixels>, &'static str); 3],
    axes_origin: Point<Pixels>,
    combination: Option<SharedString>,
}

fn to_point(x: f64, y: f64) -> Point<Pixels> {
    point(px(x as f32), px(y as f32))
}

impl Viewport {
    fn build_scene(&mut self, bounds: Bounds<Pixels>, cx: &App) -> Scene {
        let document = self.document.read(cx);
        let model = document.model();
        let width = f64::from(bounds.size.width);
        let height = f64::from(bounds.size.height);
        if self.fit_on_next_paint && width > 0.0 && height > 0.0 {
            self.fit_on_next_paint = false;
            self.camera.fit(
                model.nodes.values().map(|n| n.position.map(|v| v.si())),
                width,
                height,
            );
        }
        let centre = bounds.center();
        let centre = (f64::from(centre.x), f64::from(centre.y));
        let project = |p: [f64; 3]| {
            let (x, y, depth) = self.camera.project(p, centre);
            (to_point(x, y), depth)
        };

        let positions: std::collections::BTreeMap<EntityId, [f64; 3]> = model
            .nodes
            .iter()
            .map(|(id, n)| (*id, n.position.map(|v| v.si())))
            .collect();
        let mut snapshot = Snapshot {
            bounds,
            ..Default::default()
        };
        let nodes = model
            .nodes
            .iter()
            .map(|(id, node)| {
                let (position, _) = project(positions[id]);
                snapshot.nodes.push((*id, position));
                NodeMark {
                    position,
                    restrained: node.restrained.iter().any(|r| *r),
                    selected: document.is_selected(*id),
                    label: self.options.node_labels.then(|| node.name.clone().into()),
                }
            })
            .collect();
        let frames = model
            .frames
            .iter()
            .filter_map(|(id, frame)| {
                let a = positions.get(&frame.nodes[0])?;
                let b = positions.get(&frame.nodes[1])?;
                let (a, _) = project(*a);
                let (b, _) = project(*b);
                snapshot.frames.push((*id, a, b));
                Some(Segment {
                    a,
                    b,
                    selected: document.is_selected(*id),
                    label: self.options.frame_labels.then(|| frame.name.clone().into()),
                })
            })
            .collect();
        let mut shells: Vec<Quad> = model
            .shells
            .iter()
            .filter_map(|(id, shell)| {
                let mut points = [Point::default(); 4];
                let mut depth = 0.0;
                for (i, n) in shell.nodes.iter().enumerate() {
                    let (p, d) = project(*positions.get(n)?);
                    points[i] = p;
                    depth += d;
                }
                snapshot.shells.push((*id, points));
                Some(Quad {
                    points,
                    depth,
                    selected: document.is_selected(*id),
                })
            })
            .collect();
        shells.sort_by(|a, b| a.depth.total_cmp(&b.depth));

        let mut combination = None;
        let mut deformed_frames = vec![];
        if self.options.deformed
            && let Some(analysis) = document.analysis()
            && let Some(result) = analysis.results.combinations.get(analysis.combination)
            && let Some(displacements) = &result.displacements
        {
            combination = Some(SharedString::from(result.combination.clone()));
            let displacement = |id: EntityId| -> Option<[f64; 3]> {
                let ix = *analysis.compiled.mapping.node_index.get(&id)?;
                let d = displacements.get(ix)?;
                Some([d[0], d[1], d[2]])
            };
            let extent = model_extent(positions.values().copied());
            let max = model
                .nodes
                .keys()
                .filter_map(|id| displacement(*id))
                .map(|d| (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt())
                .fold(0.0, f64::max);
            let factor = if max > 0.0 { 0.05 * extent / max } else { 0.0 };
            let deformed = |id: EntityId| -> Option<[f64; 3]> {
                let p = positions.get(&id)?;
                let d = displacement(id)?;
                Some([
                    p[0] + d[0] * factor,
                    p[1] + d[1] * factor,
                    p[2] + d[2] * factor,
                ])
            };
            for frame in model.frames.values() {
                if let (Some(a), Some(b)) = (deformed(frame.nodes[0]), deformed(frame.nodes[1])) {
                    deformed_frames.push((project(a).0, project(b).0));
                }
            }
        }

        let axes_origin = point(bounds.left() + px(42.), bounds.bottom() - px(42.));
        let axis_end = |d: [f64; 3]| {
            let (x, y) = self.camera.project_direction(d);
            point(
                axes_origin.x + px((x * 30.0) as f32),
                axes_origin.y + px((y * 30.0) as f32),
            )
        };
        let axes = [
            (axis_end([1.0, 0.0, 0.0]), "X"),
            (axis_end([0.0, 1.0, 0.0]), "Y"),
            (axis_end([0.0, 0.0, 1.0]), "Z"),
        ];
        *self.snapshot.borrow_mut() = snapshot;
        Scene {
            bounds,
            nodes,
            frames,
            shells,
            deformed_frames,
            axes,
            axes_origin,
            combination,
        }
    }
}

fn model_extent(points: impl Iterator<Item = [f64; 3]>) -> f64 {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for p in points {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    let d: f64 = (0..3).map(|i| (max[i] - min[i]).powi(2)).sum();
    if d.is_finite() && d > 0.0 {
        d.sqrt()
    } else {
        1.0
    }
}

fn stroke_segments(
    segments: impl Iterator<Item = (Point<Pixels>, Point<Pixels>)>,
    width: Pixels,
    color: Hsla,
    window: &mut Window,
) {
    let mut builder = PathBuilder::stroke(width);
    let mut any = false;
    for (a, b) in segments {
        builder.move_to(a);
        builder.line_to(b);
        any = true;
    }
    if any && let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn paint_label(
    text: &SharedString,
    origin: Point<Pixels>,
    color: Hsla,
    style: &TextStyle,
    window: &mut Window,
    cx: &mut App,
) {
    let font_size = style.font_size.to_pixels(window.rem_size()) * 0.85;
    let run = TextRun {
        color,
        ..style.to_run(text.len())
    };
    let line = window
        .text_system()
        .shape_line(text.clone(), font_size, &[run], None);
    line.paint(origin, font_size * 1.2, TextAlign::Left, None, window, cx)
        .ok();
}

fn paint_scene(
    scene: Scene,
    palette: Palette,
    style: TextStyle,
    window: &mut Window,
    cx: &mut App,
) {
    window.with_content_mask(
        Some(ContentMask {
            bounds: scene.bounds,
        }),
        |window| {
            for quad in &scene.shells {
                let mut builder = PathBuilder::fill();
                builder.add_polygon(&quad.points, true);
                if let Ok(path) = builder.build() {
                    let color = if quad.selected {
                        palette.selected
                    } else {
                        palette.shell
                    };
                    window.paint_path(path, color.opacity(0.35));
                }
                stroke_segments(
                    (0..4).map(|i| (quad.points[i], quad.points[(i + 1) % 4])),
                    px(1.),
                    if quad.selected {
                        palette.selected
                    } else {
                        palette.shell
                    },
                    window,
                );
            }
            stroke_segments(
                scene
                    .frames
                    .iter()
                    .filter(|f| !f.selected)
                    .map(|f| (f.a, f.b)),
                px(1.5),
                palette.frame,
                window,
            );
            stroke_segments(
                scene
                    .frames
                    .iter()
                    .filter(|f| f.selected)
                    .map(|f| (f.a, f.b)),
                px(3.),
                palette.selected,
                window,
            );
            stroke_segments(
                scene.deformed_frames.iter().copied(),
                px(2.),
                palette.deformed,
                window,
            );
            for node in &scene.nodes {
                let (size_px, color) = match (node.selected, node.restrained) {
                    (true, _) => (9., palette.selected),
                    (false, true) => (8., palette.restraint),
                    (false, false) => (5., palette.node),
                };
                let bounds = Bounds::centered_at(node.position, size(px(size_px), px(size_px)));
                if node.restrained && !node.selected {
                    window.paint_quad(quad(
                        bounds,
                        Corners::default(),
                        palette.restraint.opacity(0.25),
                        Edges::all(px(1.5)),
                        palette.restraint,
                        BorderStyle::Solid,
                    ));
                } else {
                    window.paint_quad(fill(bounds, color));
                }
            }
            for node in &scene.nodes {
                if let Some(label) = &node.label {
                    let origin = point(node.position.x + px(6.), node.position.y - px(14.));
                    paint_label(label, origin, palette.label, &style, window, cx);
                }
            }
            for frame in &scene.frames {
                if let Some(label) = &frame.label {
                    let mid = point(
                        (frame.a.x + frame.b.x) / 2. + px(4.),
                        (frame.a.y + frame.b.y) / 2. - px(8.),
                    );
                    paint_label(label, mid, palette.label, &style, window, cx);
                }
            }
            for (i, (end, name)) in scene.axes.iter().enumerate() {
                stroke_segments(
                    std::iter::once((scene.axes_origin, *end)),
                    px(2.),
                    palette.axes[i],
                    window,
                );
                let label = SharedString::new_static(name);
                let origin = point(end.x + px(3.), end.y - px(8.));
                paint_label(&label, origin, palette.axes[i], &style, window, cx);
            }
            if let Some(combination) = &scene.combination {
                let text: SharedString = format!("Deformed shape: {combination}").into();
                let origin = point(scene.bounds.left() + px(12.), scene.bounds.top() + px(8.));
                paint_label(&text, origin, palette.deformed, &style, window, cx);
            }
        },
    );
}

impl Render for Viewport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let palette = Palette {
            frame: theme.foreground.opacity(0.8),
            shell: theme.chart_2,
            node: theme.muted_foreground,
            restraint: theme.chart_3,
            selected: theme.primary,
            deformed: theme.chart_1,
            label: theme.muted_foreground,
            axes: [theme.red, theme.green, theme.blue],
        };
        let background = theme.background;
        let hint_color = theme.muted_foreground;
        let style = window.text_style();
        let view = cx.entity().downgrade();
        let selection = self.document.read(cx).selection().len();
        let kind_counts = {
            let model = self.document.read(cx).model();
            (model.nodes.len(), model.frames.len(), model.shells.len())
        };
        div()
            .id("viewport")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(background)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up_out))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::on_mouse_up_out))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::on_mouse_up_out))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        view.update(cx, |viewport, cx| viewport.build_scene(bounds, cx))
                            .ok()
                    },
                    move |_, scene, window, cx| {
                        if let Some(scene) = scene {
                            paint_scene(scene, palette, style, window, cx);
                        }
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .bottom_2()
                    .right_3()
                    .text_xs()
                    .text_color(hint_color)
                    .child(format!(
                        "{} nodes · {} frames · {} shells · {} selected    right-drag orbit · shift+right or middle-drag pan · wheel zoom · click select · shift+click extend",
                        kind_counts.0, kind_counts.1, kind_counts.2, selection
                    )),
            )
    }
}
