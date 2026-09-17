//! The 3D model view: an orthographic wireframe of nodes, frames, and
//! shells painted on a canvas, with orbit, pan, zoom, and click selection.
//! The draw tools live here too: Node places a node where you click, Frame
//! joins two clicked nodes, Shell four. Finished shapes are reported as
//! [`ViewportEvent`]s for the workspace to turn into commands. The view
//! controls sit in the top-right corner of the canvas, and an empty model
//! shows a start card instead of a blank canvas.
use crate::actions::*;
use crate::camera::{Camera, UpAxis, ViewPreset};
use crate::document::Document;
use crate::results::{Diagram, labelled_stations, peak};
use crate::text::{UNITS, fmt_q};
use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, Theme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use oa_model::EntityId;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DisplayOptions {
    pub node_labels: bool,
    pub frame_labels: bool,
    pub deformed: bool,
    /// The section force drawn along every member, if any.
    pub diagram: Option<Diagram>,
}

/// What a click in the view does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    /// Click empty space to place a node on the ground plane.
    Node,
    /// Click node I, then node J. The next frame starts from J.
    Frame,
    /// Click four nodes in order around the shell.
    Shell,
}

impl Tool {
    /// Nodes a shape needs before it is complete.
    pub fn picks(self) -> usize {
        match self {
            Tool::Frame => 2,
            Tool::Shell => 4,
            Tool::Select | Tool::Node => 0,
        }
    }
}

/// A shape finished with a draw tool, for the workspace to add to the model,
/// or a double-click asking for the selection's properties.
pub enum ViewportEvent {
    PlaceNode([f64; 3]),
    DrawFrame([EntityId; 2]),
    DrawShell([EntityId; 4]),
    OpenProperties,
}

/// Grid the Node tool snaps to: one foot, held in metres like the model.
const SNAP: f64 = 0.3048;

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
    /// The preset the camera was last set to, until the user orbits away.
    preset: Option<ViewPreset>,
    options: DisplayOptions,
    tool: Tool,
    /// Nodes the Frame or Shell tool has taken so far, in click order.
    picked: Vec<EntityId>,
    /// Pointer position while it is over the view and no drag is running.
    hover: Option<Point<Pixels>>,
    hover_node: Option<EntityId>,
    focus_handle: FocusHandle,
    snapshot: Rc<RefCell<Snapshot>>,
    drag: Option<Drag>,
    fit_on_next_paint: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ViewportEvent> for Viewport {}

impl Viewport {
    pub fn new(document: Entity<Document>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&document, |this, document, cx| {
            let model = document.read(cx).model();
            this.picked.retain(|id| model.kind_of(*id).is_some());
            cx.notify();
        });
        Self {
            document,
            camera: Camera::default(),
            preset: Some(ViewPreset::ThreeD),
            options: DisplayOptions::default(),
            tool: Tool::Select,
            picked: vec![],
            hover: None,
            hover_node: None,
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
    pub fn preset(&self) -> Option<ViewPreset> {
        self.preset
    }
    pub fn tool(&self) -> Tool {
        self.tool
    }
    pub fn picked(&self) -> &[EntityId] {
        &self.picked
    }
    /// Focus here puts the single-key bindings in reach.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }
    pub fn set_options(&mut self, options: DisplayOptions, cx: &mut Context<Self>) {
        self.options = options;
        cx.notify();
    }
    pub fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if self.tool != tool {
            self.tool = tool;
            self.picked.clear();
            cx.notify();
        }
    }
    /// Escape: drops the nodes picked so far, or else returns to Select.
    /// False when there was nothing to cancel.
    pub fn cancel(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.picked.is_empty() {
            self.picked.clear();
            cx.notify();
            return true;
        }
        if self.tool != Tool::Select {
            self.tool = Tool::Select;
            cx.notify();
            return true;
        }
        false
    }
    pub fn set_preset(&mut self, preset: ViewPreset, cx: &mut Context<Self>) {
        self.camera.set_preset(preset);
        self.preset = Some(preset);
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
            let node = self.node_at(event.position);
            let changed = node != self.hover_node;
            self.hover = Some(event.position);
            self.hover_node = node;
            // Draw tools follow the pointer; Select only cares which node is under it.
            if changed || self.tool != Tool::Select {
                cx.notify();
            }
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
            MouseButton::Right if !drag.shift => {
                self.camera.orbit(dx, dy);
                self.preset = None;
            }
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
            self.click(event.position, event.modifiers.shift, event.click_count, cx);
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

    /// A left click that did not drag: the current tool decides what it means.
    /// A double-click with Select opens the properties of what was hit.
    fn click(
        &mut self,
        position: Point<Pixels>,
        extend: bool,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        match self.tool {
            Tool::Select => {
                self.pick(position, extend, cx);
                if click_count == 2 && !self.document.read(cx).selection().is_empty() {
                    cx.emit(ViewportEvent::OpenProperties);
                }
            }
            Tool::Node => match self.node_at(position) {
                Some(id) => self
                    .document
                    .update(cx, |document, cx| document.set_selection(vec![id], cx)),
                None => {
                    let world = self.ground_point(position);
                    cx.emit(ViewportEvent::PlaceNode(world));
                }
            },
            Tool::Frame | Tool::Shell => {
                let Some(id) = self.node_at(position) else {
                    return;
                };
                if self.picked.last() == Some(&id) {
                    return;
                }
                self.picked.push(id);
                if self.picked.len() == self.tool.picks() {
                    match self.tool {
                        Tool::Frame => {
                            let (a, b) = (self.picked[0], self.picked[1]);
                            cx.emit(ViewportEvent::DrawFrame([a, b]));
                            self.picked = vec![b];
                        }
                        _ => {
                            let nodes = [
                                self.picked[0],
                                self.picked[1],
                                self.picked[2],
                                self.picked[3],
                            ];
                            cx.emit(ViewportEvent::DrawShell(nodes));
                            self.picked.clear();
                        }
                    }
                }
                cx.notify();
            }
        }
    }

    /// Where the Node tool would put a node for a pointer position.
    fn ground_point(&self, position: Point<Pixels>) -> [f64; 3] {
        let bounds = self.snapshot.borrow().bounds;
        snap_to_ground(&self.camera, position, bounds)
    }

    /// The node within reach of the pointer, nearest first.
    fn node_at(&self, position: Point<Pixels>) -> Option<EntityId> {
        let snapshot = self.snapshot.borrow();
        let p = to_f64(position);
        snapshot
            .nodes
            .iter()
            .map(|(id, q)| (*id, distance(p, to_f64(*q))))
            .filter(|(_, d)| *d <= 8.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// Selects the entity under the pointer: nodes first, then frames, then shells.
    fn pick(&mut self, position: Point<Pixels>, extend: bool, cx: &mut Context<Self>) {
        let hit = self.node_at(position).or_else(|| {
            let snapshot = self.snapshot.borrow();
            let p = to_f64(position);
            snapshot
                .frames
                .iter()
                .map(|(id, a, b)| (*id, segment_distance(p, to_f64(*a), to_f64(*b))))
                .filter(|(_, d)| *d <= 6.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(id, _)| id)
                .or_else(|| {
                    snapshot
                        .shells
                        .iter()
                        .rev()
                        .find(|(_, pts)| point_in_polygon(p, &pts.map(to_f64)))
                        .map(|(id, _)| *id)
                })
        });
        self.document
            .update(cx, |document, cx| match (hit, extend) {
                (Some(id), true) => document.toggle_selected(id, cx),
                (Some(id), false) => document.set_selection(vec![id], cx),
                (None, false) => document.clear_selection(cx),
                (None, true) => {}
            });
    }
}

/// The ground-plane point under the pointer, snapped to the grid. In an
/// elevation the ground is edge-on, so the point lies in the view plane
/// through the centre of the view instead.
fn snap_to_ground(camera: &Camera, position: Point<Pixels>, bounds: Bounds<Pixels>) -> [f64; 3] {
    let centre = bounds.center();
    let axis = match camera.up {
        UpAxis::Y => 1,
        UpAxis::Z => 2,
    };
    let p = camera.unproject(to_f64(position), to_f64(centre), Some((axis, 0.0)));
    p.map(|v| (v / SNAP).round() * SNAP)
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

pub fn preset_action(preset: ViewPreset) -> Box<dyn Action> {
    match preset {
        ViewPreset::ThreeD => Box::new(ViewThreeD),
        ViewPreset::Plan => Box::new(ViewPlan),
        ViewPreset::ElevationX => Box::new(ViewElevationX),
        ViewPreset::ElevationY => Box::new(ViewElevationY),
    }
}

// MARK: Painting

struct Palette {
    frame: Hsla,
    shell: Hsla,
    node: Hsla,
    restraint: Hsla,
    selected: Hsla,
    deformed: Hsla,
    diagram: Hsla,
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
/// One member's section-force diagram, offset from it in its plane of bending.
struct DiagramShape {
    /// The member from I to J, then the curve back from J to I, for the fill.
    outline: Vec<Point<Pixels>>,
    curve: Vec<Point<Pixels>>,
    /// Values at the ends and the peak, in display units.
    labels: Vec<(Point<Pixels>, SharedString)>,
}

/// Everything the paint closure needs, computed once per frame in prepaint.
struct Scene {
    bounds: Bounds<Pixels>,
    nodes: Vec<NodeMark>,
    frames: Vec<Segment>,
    shells: Vec<Quad>,
    deformed_frames: Vec<(Point<Pixels>, Point<Pixels>)>,
    diagrams: Vec<DiagramShape>,
    axes: [(Point<Pixels>, &'static str); 3],
    axes_origin: Point<Pixels>,
    combination: Option<SharedString>,
    /// What the diagrams show and for which combination.
    diagram_legend: Option<SharedString>,
    /// Nodes the draw tool has taken, ringed in the accent colour.
    picked: Vec<Point<Pixels>>,
    /// The node under the pointer while a draw tool is active.
    hover_node: Option<Point<Pixels>>,
    /// Where the line from the last picked node is heading.
    rubber: Option<Point<Pixels>>,
    /// Where the Node tool would place a node.
    ghost: Option<Point<Pixels>>,
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

        // Section-force diagrams, scaled so the largest value in the model
        // stands 8% of its extent off the member.
        let mut diagram_legend = None;
        let mut diagrams = vec![];
        if let Some(which) = self.options.diagram
            && let Some(analysis) = document.analysis()
            && let Some(result) = analysis.results.combinations.get(analysis.combination)
        {
            let shown = analysis.shown_diagrams();
            let column = which.index();
            let max = peak(shown, column);
            let extent = model_extent(positions.values().copied());
            let factor = if max > 0.0 { 0.08 * extent / max } else { 0.0 };
            diagram_legend = Some(SharedString::from(format!(
                "{} ({}): {}",
                which.label(),
                UNITS.symbol(which.role()),
                result.combination
            )));
            for (id, frame) in &model.frames {
                let Some(diagram) = analysis
                    .compiled
                    .mapping
                    .frame_index
                    .get(id)
                    .and_then(|ix| shown.get(*ix)?.as_ref())
                else {
                    continue;
                };
                let Some(origin) = positions.get(&frame.nodes[0]) else {
                    continue;
                };
                let (along, normal) = (diagram.axes[0], diagram.axes[which.axis()]);
                let values: Vec<f64> = diagram.forces.iter().map(|f| f[column]).collect();
                let world = |x: f64, value: f64| {
                    let d = value * factor;
                    [
                        origin[0] + along[0] * x + normal[0] * d,
                        origin[1] + along[1] * x + normal[1] * d,
                        origin[2] + along[2] * x + normal[2] * d,
                    ]
                };
                let curve: Vec<Point<Pixels>> = diagram
                    .stations
                    .iter()
                    .zip(&values)
                    .map(|(x, v)| project(world(*x, *v)).0)
                    .collect();
                let mut outline = vec![
                    project(*origin).0,
                    project(world(diagram.length, 0.0)).0,
                ];
                outline.extend(curve.iter().rev().copied());
                let labels = labelled_stations(&values, max)
                    .into_iter()
                    .map(|k| (curve[k], SharedString::from(fmt_q(which.role(), values[k]))))
                    .collect();
                diagrams.push(DiagramShape {
                    outline,
                    curve,
                    labels,
                });
            }
        }

        // Draw-tool feedback.
        let picked: Vec<Point<Pixels>> = self
            .picked
            .iter()
            .filter_map(|id| positions.get(id).map(|p| project(*p).0))
            .collect();
        let hover_node = match self.tool {
            Tool::Select => None,
            _ => self
                .hover_node
                .and_then(|id| positions.get(&id).map(|p| project(*p).0)),
        };
        let rubber = match self.tool {
            Tool::Frame | Tool::Shell if !picked.is_empty() => hover_node.or(self.hover),
            _ => None,
        };
        let ghost = match (self.tool, self.hover_node, self.hover) {
            (Tool::Node, None, Some(hover)) => {
                Some(project(snap_to_ground(&self.camera, hover, bounds)).0)
            }
            _ => None,
        };

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
            diagrams,
            axes,
            axes_origin,
            combination,
            diagram_legend,
            picked,
            hover_node,
            rubber,
            ghost,
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

pub(crate) fn stroke_segments(
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

/// A hollow circle, for nodes the draw tool has taken or is about to take.
fn paint_ring(centre: Point<Pixels>, diameter: f32, width: f32, color: Hsla, window: &mut Window) {
    let bounds = Bounds::centered_at(centre, size(px(diameter), px(diameter)));
    window.paint_quad(quad(
        bounds,
        Corners::all(px(diameter / 2.)),
        transparent_black(),
        Edges::all(px(width)),
        color,
        BorderStyle::Solid,
    ));
}

pub(crate) fn paint_label(
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
            for shape in &scene.diagrams {
                let mut builder = PathBuilder::fill();
                builder.add_polygon(&shape.outline, true);
                if let Ok(path) = builder.build() {
                    window.paint_path(path, palette.diagram.opacity(0.2));
                }
                stroke_segments(
                    shape.curve.windows(2).map(|w| (w[0], w[1])),
                    px(1.5),
                    palette.diagram,
                    window,
                );
            }
            // The shape being drawn: picked nodes joined, then a line to the pointer.
            stroke_segments(
                scene.picked.windows(2).map(|w| (w[0], w[1])),
                px(2.),
                palette.selected,
                window,
            );
            if let (Some(last), Some(target)) = (scene.picked.last(), scene.rubber) {
                stroke_segments(
                    std::iter::once((*last, target)),
                    px(1.5),
                    palette.selected.opacity(0.7),
                    window,
                );
            }
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
            for p in &scene.picked {
                paint_ring(*p, 14., 2., palette.selected, window);
            }
            if let Some(p) = scene.hover_node {
                paint_ring(p, 12., 1.5, palette.selected.opacity(0.8), window);
            }
            if let Some(p) = scene.ghost {
                let bounds = Bounds::centered_at(p, size(px(7.), px(7.)));
                window.paint_quad(fill(bounds, palette.selected.opacity(0.6)));
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
            for shape in &scene.diagrams {
                for (p, text) in &shape.labels {
                    let origin = point(p.x + px(3.), p.y - px(14.));
                    paint_label(text, origin, palette.diagram, &style, window, cx);
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
            let mut legend_top = scene.bounds.top() + px(8.);
            if let Some(combination) = &scene.combination {
                let text: SharedString = format!("Deformed shape: {combination}").into();
                let origin = point(scene.bounds.left() + px(12.), legend_top);
                paint_label(&text, origin, palette.deformed, &style, window, cx);
                legend_top += px(16.);
            }
            if let Some(legend) = &scene.diagram_legend {
                let origin = point(scene.bounds.left() + px(12.), legend_top);
                paint_label(legend, origin, palette.diagram, &style, window, cx);
            }
        },
    );
}

// MARK: Overlays

impl Viewport {
    /// View presets, fit, and the up axis in the top-right corner, each
    /// labelled with its key. Text only; the key map is the affordance.
    fn view_controls(&self) -> AnyElement {
        let preset = self.preset;
        let up = self.camera.up;
        let presets = [
            ("view-3d", "3D  1", "Look from above and to the side", ViewPreset::ThreeD),
            ("view-plan", "Plan  2", "Look straight down", ViewPreset::Plan),
            ("view-x", "Elev X  3", "Elevation with X across the screen", ViewPreset::ElevationX),
            ("view-y", "Elev Y  4", "Elevation with Y across the screen", ViewPreset::ElevationY),
        ];
        let camera = ButtonGroup::new("view-presets")
            .small()
            .outline()
            .children(presets.iter().map(|(id, label, description, which)| {
                let action = preset_action(*which);
                Button::new(*id)
                    .label(*label)
                    .selected(preset == Some(*which))
                    .tooltip_with_action(*description, action.as_ref(), None)
            }))
            .on_click(move |clicks, window, cx| {
                if let Some(ix) = clicks.first()
                    && let Some((_, _, _, which)) = presets.get(*ix)
                {
                    window.dispatch_action(preset_action(*which), cx);
                }
            });
        let fit = Button::new("fit")
            .small()
            .outline()
            .label("Fit  Z")
            .tooltip_with_action("Fit the whole model in the view", &ZoomExtents, None)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ZoomExtents), cx));
        let up_axis = ButtonGroup::new("up-axis")
            .small()
            .outline()
            .child(
                Button::new("up-y")
                    .label("Y up")
                    .selected(up == UpAxis::Y)
                    .tooltip_with_action("Draw Y as the vertical axis", &ToggleUpAxis, None),
            )
            .child(
                Button::new("up-z")
                    .label("Z up")
                    .selected(up == UpAxis::Z)
                    .tooltip_with_action("Draw Z as the vertical axis", &ToggleUpAxis, None),
            )
            .on_click(move |clicks, window, cx| {
                let want = if clicks.first() == Some(&0) {
                    UpAxis::Y
                } else {
                    UpAxis::Z
                };
                if want != up {
                    window.dispatch_action(Box::new(ToggleUpAxis), cx);
                }
            });
        h_flex()
            .absolute()
            .top_4()
            .right_4()
            .gap_2()
            .occlude()
            .child(camera)
            .child(fit)
            .child(up_axis)
            .into_any_element()
    }
}

/// Three ways to start and the order the menus read in, each with its key,
/// shown over an empty model. Text only, on the 8px grid.
fn start_card(theme: &Theme) -> AnyElement {
    let (fg, muted, border, surface, primary, panel) = (
        theme.foreground,
        theme.muted_foreground,
        theme.border,
        theme.background,
        theme.primary,
        theme.sidebar,
    );
    let key = move |text: &'static str, color: Hsla| {
        div()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .whitespace_nowrap()
            .child(text)
    };
    let choice = move |id: &'static str,
                       label: &'static str,
                       shortcut: &'static str,
                       featured: bool,
                       action: Box<dyn Action>| {
        h_flex()
            .id(id)
            .h(px(40.))
            .px_4()
            .items_center()
            .justify_between()
            .rounded(px(8.))
            .border_1()
            .border_color(if featured { primary } else { border })
            .bg(surface)
            .text_sm()
            .text_color(fg)
            .cursor_pointer()
            .hover(move |s| s.border_color(primary))
            .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
            .child(label)
            .child(key(shortcut, if featured { primary } else { muted }))
    };
    let step = move |what: &'static str, keys: &'static str| {
        h_flex()
            .h(px(24.))
            .items_center()
            .justify_between()
            .text_sm()
            .text_color(fg)
            .child(what)
            .child(key(keys, muted))
    };
    v_flex()
        .absolute()
        .inset_0()
        .items_center()
        .justify_center()
        .child(
            v_flex()
                .id("start-card")
                .occlude()
                .w(px(480.))
                .gap_6()
                .p_8()
                .rounded(px(8.))
                .border_1()
                .border_color(border)
                .bg(panel)
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(20.))
                                .line_height(px(24.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg)
                                .child("Start a model"),
                        )
                        .child(div().text_sm().text_color(muted).child(
                            "Every step is undoable. The menus read left to right in the order a model is built.",
                        )),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(choice(
                            "start-example",
                            "Example frame · two storeys, loaded",
                            "Ctrl+Shift+N",
                            true,
                            Box::new(NewExampleModel),
                        ))
                        .child(choice(
                            "start-node",
                            "Place nodes by clicking in the view",
                            "N",
                            false,
                            Box::new(NodeTool),
                        ))
                        .child(choice(
                            "start-open",
                            "Open a saved model",
                            "Ctrl+O",
                            false,
                            Box::new(OpenModel),
                        )),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .pt_4()
                        .border_t_1()
                        .border_color(border)
                        .child(step("1  Define a material and a section", "Ctrl+M · Ctrl+T"))
                        .child(step("2  Draw nodes, then frames and shells", "N · F · S"))
                        .child(step("3  Assign a load case and loads", "Ctrl+L · L · U"))
                        .child(step("4  Run and show the deformed shape", "Ctrl+R · Shift+D")),
                ),
        )
        .into_any_element()
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
            diagram: theme.warning,
            label: theme.muted_foreground,
            axes: [theme.red, theme.green, theme.blue],
        };
        let background = theme.background;
        let hint_color = theme.muted_foreground;
        let empty = self.document.read(cx).model().nodes.is_empty();
        let card = empty.then(|| start_card(theme));
        let controls = self.view_controls();
        let style = window.text_style();
        let view = cx.entity().downgrade();
        let drawing = self.tool != Tool::Select;
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(background)
            .child(
                div()
                    .id("viewport")
                    .key_context("Viewport")
                    .track_focus(&self.focus_handle)
                    .size_full()
                    .when(drawing, |this| this.cursor_crosshair())
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
                    ),
            )
            .child(controls)
            .children(card)
            .child(
                h_flex()
                    .absolute()
                    .bottom_4()
                    .right_4()
                    .gap_4()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(hint_color)
                    .child("Right-drag orbit")
                    .child("Middle-drag pan")
                    .child("Wheel zoom"),
            )
    }
}
