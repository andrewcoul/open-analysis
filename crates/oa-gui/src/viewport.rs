//! The 3D model view: an orthographic wireframe of nodes, frames, and
//! shells painted on a canvas, with orbit, pan, zoom, and click selection.
//! The draw tools live here too: Node places a node on the active level
//! where you click, Frame joins two clicked nodes, Shell four. All three
//! snap to the frames, shell edges, and underlay lines in view (see
//! [`crate::snap`]), and Frame and Shell make a node where a snapped click
//! finds none. Finished shapes are reported as [`ViewportEvent`]s for the workspace to turn into
//! commands. The view can show the whole model or one level's floor, with
//! or without the storeys beside it as context; the active level is also
//! the working plane the Node tool places on. The view controls sit in the
//! top-right corner of the canvas, and an empty model shows a start card
//! instead of a blank canvas.
use crate::actions::*;
use crate::camera::{Camera, UpAxis, ViewPreset};
use crate::document::Document;
use crate::results::{Diagram, labelled_stations, peak};
use crate::snap;
use crate::text::{UNITS, fmt_q};
use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _, Theme, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use oa_model::levels::{self, Membership};
use oa_model::{EntityId, EntityKind, Model, Role};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DisplayOptions {
    pub node_labels: bool,
    pub frame_labels: bool,
    pub deformed: bool,
    /// The section force drawn along every member, if any.
    pub diagram: Option<Diagram>,
    /// Inverted so that the default shows them.
    pub hide_underlays: bool,
    /// The object snaps the draw tools use.
    pub snaps: snap::Modes,
}

/// What a click in the view does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    /// Click empty space to place a node on the active level.
    Node,
    /// Click node I, then node J, or snap points to make them at. The next
    /// frame starts from J.
    Frame,
    /// Click four nodes or snap points in order around the shell.
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

/// How much of the model the view shows. The active level decides what a
/// level view holds and where the Node tool places, whatever the mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Whole,
    /// The active level's floor objects, plus a marker where each member
    /// spanning to another level meets its plane.
    Level,
    /// The active level with the storeys above and below drawn subdued;
    /// context cannot be picked or snapped to.
    LevelContext,
}

/// A corner of the shape being drawn: a node the model has, or a snapped
/// point on a level where one is made when the shape is finished.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Node(EntityId),
    Point { position: [f64; 3], level: EntityId },
}

/// A shape finished with a draw tool, for the workspace to add to the model,
/// or a double-click asking for the selection's properties.
pub enum ViewportEvent {
    /// A node on a level, at that level's exact elevation.
    PlaceNode { position: [f64; 3], level: EntityId },
    DrawFrame([Pick; 2]),
    DrawShell([Pick; 4]),
    OpenProperties,
}

/// Plan grid the Node tool snaps X and Y to when no object snap is in reach:
/// one foot, held in metres like the model. Z is never snapped; it is the
/// level's elevation.
const SNAP: f64 = 0.3048;

/// Screen positions of the last painted frame, used for picking. Only what
/// the view shows is here, so hidden or context geometry cannot be picked.
#[derive(Default)]
struct Snapshot {
    bounds: Bounds<Pixels>,
    nodes: Vec<(EntityId, Point<Pixels>)>,
    frames: Vec<(EntityId, Point<Pixels>, Point<Pixels>)>,
    shells: Vec<(EntityId, [Point<Pixels>; 4])>,
    /// Lines the draw tools snap to: frames, shell edges, and underlays.
    /// Empty while nothing is being drawn.
    segments: Vec<snap::Segment>,
}

/// Where a draw-tool click that misses every node would land.
struct Aim {
    position: [f64; 3],
    level: EntityId,
    snap: Option<snap::Hit>,
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
    mode: ViewMode,
    /// The level the Node tool places on and the level views show. Resolved
    /// against the model whenever it changes; None only for a model with no
    /// levels, which the model layer does not allow.
    active_level: Option<EntityId>,
    tool: Tool,
    /// Corners the Frame or Shell tool has taken so far, in click order.
    picked: Vec<Pick>,
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
            this.resolve_picks(model);
            this.resolve_level(model);
            cx.notify();
        });
        let active_level = document.read(cx).model().base_level();
        Self {
            document,
            camera: Camera::default(),
            preset: Some(ViewPreset::ThreeD),
            options: DisplayOptions::default(),
            mode: ViewMode::Whole,
            active_level,
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
    pub fn mode(&self) -> ViewMode {
        self.mode
    }
    pub fn active_level(&self) -> Option<EntityId> {
        self.active_level
    }
    /// Whether a click can land on the active level's plane from here.
    pub fn plane_visible(&self) -> bool {
        self.camera.plane_visible(2)
    }
    pub fn tool(&self) -> Tool {
        self.tool
    }
    pub fn picked(&self) -> &[Pick] {
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

    // MARK: Levels

    /// Keeps the active level pointing at a level the model has: after an
    /// open, an undo, or a deletion it falls back to the lowest level.
    fn resolve_level(&mut self, model: &Model) {
        if self
            .active_level
            .is_none_or(|l| !model.levels.contains_key(&l))
        {
            self.active_level = model.base_level();
        }
    }
    /// Keeps the corners taken so far pointing at what the model has: a
    /// deleted node goes, and a point where a node now stands becomes it.
    fn resolve_picks(&mut self, model: &Model) {
        self.picked.retain(|pick| match pick {
            Pick::Node(id) => model.nodes.contains_key(id),
            Pick::Point { level, .. } => model.levels.contains_key(level),
        });
        for pick in &mut self.picked {
            if let Pick::Point { position, .. } = *pick
                && let Some(id) = node_at_position(model, position)
            {
                *pick = Pick::Node(id);
            }
        }
    }
    /// Makes a level the active one. A partly drawn shape is dropped, and
    /// geometry the new view hides leaves the selection, so an off-screen
    /// entity cannot take an assignment meant for what is shown.
    pub fn set_active_level(&mut self, level: EntityId, cx: &mut Context<Self>) {
        if self.active_level == Some(level)
            || !self.document.read(cx).model().levels.contains_key(&level)
        {
            return;
        }
        self.active_level = Some(level);
        self.picked.clear();
        self.prune_selection(cx);
        cx.notify();
    }
    /// The level above (+1) or below (-1) the active one, stopping at the ends.
    pub fn step_level(&mut self, step: isize, cx: &mut Context<Self>) {
        let order = self.document.read(cx).model().levels_by_elevation();
        let Some(current) = self
            .active_level
            .and_then(|l| order.iter().position(|x| *x == l))
        else {
            return;
        };
        let last = order.len() as isize - 1;
        let next = (current as isize + step).clamp(0, last) as usize;
        self.set_active_level(order[next], cx);
    }
    /// A level is a plane of constant Z, so a level view looks down Z from
    /// above whatever up axis the 3D view was drawn with.
    pub fn set_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        if mode != ViewMode::Whole {
            self.camera.up = UpAxis::Z;
            self.camera.set_preset(ViewPreset::Plan);
            self.preset = Some(ViewPreset::Plan);
            self.fit_on_next_paint = true;
        }
        self.picked.clear();
        self.prune_selection(cx);
        cx.notify();
    }
    /// The objects the view shows, or None when it shows the whole model.
    fn shown(&self, model: &Model) -> Option<Membership> {
        match (self.mode, self.active_level) {
            (ViewMode::Whole, _) | (_, None) => None,
            (_, Some(level)) => Some(levels::membership(model, level)),
        }
    }
    /// Everything Select All takes: the whole model, or in a level view only
    /// the floor objects the view lets the user pick, so a hidden storey
    /// never rides along into a delete or an assignment.
    pub fn selectable(&self, model: &Model) -> Vec<EntityId> {
        match self.shown(model) {
            None => model
                .nodes
                .keys()
                .chain(model.frames.keys())
                .chain(model.shells.keys())
                .copied()
                .collect(),
            Some(shown) => shown
                .nodes
                .iter()
                .chain(&shown.frames)
                .chain(&shown.shells)
                .copied()
                .collect(),
        }
    }
    fn prune_selection(&self, cx: &mut Context<Self>) {
        let Some(shown) = self.shown(self.document.read(cx).model()) else {
            return;
        };
        self.document.update(cx, |document, cx| {
            let model = document.model();
            let kept: Vec<EntityId> = document
                .selection()
                .iter()
                .copied()
                .filter(|id| match model.kind_of(*id) {
                    Some(EntityKind::Node) => shown.nodes.contains(id),
                    Some(EntityKind::Frame) => shown.frames.contains(id),
                    Some(EntityKind::Shell) => shown.shells.contains(id),
                    _ => true,
                })
                .collect();
            if kept.len() != document.selection().len() {
                document.set_selection(kept, cx);
            }
        });
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
                    // No placement when the plane is edge-on: the prompt says so.
                    let aim = self.aim(position, self.document.read(cx).model());
                    if let Some(Aim { position, level, .. }) = aim {
                        cx.emit(ViewportEvent::PlaceNode { position, level });
                    }
                }
            },
            Tool::Frame | Tool::Shell => {
                let pick = match self.node_at(position) {
                    Some(id) => Pick::Node(id),
                    None => match self.aim(position, self.document.read(cx).model()) {
                        Some(Aim { position, level, .. }) => Pick::Point { position, level },
                        None => return,
                    },
                };
                if self.picked.last() == Some(&pick) {
                    return;
                }
                self.picked.push(pick);
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

    /// Where the Node tool would put a node for a pointer position: on the
    /// active level's plane, X and Y snapped to the plan grid and Z exactly
    /// the level's elevation. None without a level, or when the plane is
    /// edge-on, since a point in the view plane would not lie on the level.
    fn work_plane_point(
        &self,
        position: Point<Pixels>,
        model: &Model,
    ) -> Option<([f64; 3], EntityId)> {
        let level = self.active_level?;
        let elevation = model.levels.get(&level)?.elevation.si();
        if !self.camera.plane_visible(2) {
            return None;
        }
        let centre = self.snapshot.borrow().bounds.center();
        let p = self
            .camera
            .unproject(to_f64(position), to_f64(centre), Some((2, elevation)));
        let snap = |v: f64| (v / SNAP).round() * SNAP;
        Some(([snap(p[0]), snap(p[1]), elevation], level))
    }

    /// Where a draw-tool click that misses every node would land: the object
    /// snap in reach, brought onto the active level in plan, or for the Node
    /// tool the plan grid. Frame and Shell make nodes only at snap points.
    /// A perpendicular is dropped from the last corner taken.
    fn aim(&self, position: Point<Pixels>, model: &Model) -> Option<Aim> {
        let (grid, level) = self.work_plane_point(position, model)?;
        let from = self.picked.last().and_then(|pick| match pick {
            Pick::Node(id) => model.nodes.get(id).map(|n| n.position.map(|v| v.si())),
            Pick::Point { position, .. } => Some(*position),
        });
        let snap = snap::find(
            &self.snapshot.borrow().segments,
            to_f64(position),
            from.map(|p| [p[0], p[1]]),
            self.options.snaps,
        );
        match (snap, self.tool) {
            (Some(hit), _) => Some(Aim {
                position: [hit.world[0], hit.world[1], grid[2]],
                level,
                snap,
            }),
            (None, Tool::Node) => Some(Aim {
                position: grid,
                level,
                snap: None,
            }),
            (None, _) => None,
        }
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

/// The node standing at a position, to a micron, so a snapped point on an
/// existing node takes it rather than stacking a second one there.
pub fn node_at_position(model: &Model, position: [f64; 3]) -> Option<EntityId> {
    model
        .nodes
        .iter()
        .find(|(_, node)| (0..3).all(|i| (node.position[i].si() - position[i]).abs() < 1e-6))
        .map(|(id, _)| *id)
}

/// The point between two member ends at a plane of constant Z, when the
/// member crosses it.
fn plane_crossing(a: [f64; 3], b: [f64; 3], elevation: f64) -> Option<[f64; 3]> {
    let rise = b[2] - a[2];
    if rise.abs() < 1e-12 {
        return None;
    }
    let t = (elevation - a[2]) / rise;
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    Some([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), elevation])
}

/// Where a quadrilateral meets a plane of constant Z: the two edge crossings
/// furthest apart, or none when the plane misses it or only touches a corner.
fn shell_trace(corners: [[f64; 3]; 4], elevation: f64) -> Option<([f64; 3], [f64; 3])> {
    let cuts: Vec<[f64; 3]> = (0..4)
        .filter_map(|i| plane_crossing(corners[i], corners[(i + 1) % 4], elevation))
        .collect();
    let apart = |p: &[f64; 3], q: &[f64; 3]| (p[0] - q[0]).hypot(p[1] - q[1]);
    let mut best: Option<(&[f64; 3], &[f64; 3])> = None;
    for (i, p) in cuts.iter().enumerate() {
        for q in &cuts[i + 1..] {
            if best.is_none_or(|(a, b)| apart(p, q) > apart(a, b)) {
                best = Some((p, q));
            }
        }
    }
    best.filter(|(p, q)| apart(p, q) > 1e-9)
        .map(|(p, q)| (*p, *q))
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
    /// Storeys shown beside the active level, and members spanning to them.
    context: Hsla,
    underlay: Hsla,
    snap: Hsla,
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
    /// Geometry of the storeys beside the active level, drawn subdued.
    context_nodes: Vec<Point<Pixels>>,
    context_frames: Vec<(Point<Pixels>, Point<Pixels>)>,
    context_shells: Vec<[Point<Pixels>; 4]>,
    /// CAD drawings on the levels in view. Reference only: never picked.
    underlays: Vec<(Point<Pixels>, Point<Pixels>)>,
    /// Where members spanning to another level meet the active plane. Not
    /// nodes: they cannot be picked or snapped to.
    crossings: Vec<Point<Pixels>>,
    /// Edges of spanning shells that lie along the active level: a wall's
    /// trace on the floor.
    traces: Vec<(Point<Pixels>, Point<Pixels>)>,
    deformed_frames: Vec<(Point<Pixels>, Point<Pixels>)>,
    diagrams: Vec<DiagramShape>,
    axes: [(Point<Pixels>, &'static str); 3],
    axes_origin: Point<Pixels>,
    /// The level a level view shows, with its elevation.
    level_legend: Option<SharedString>,
    combination: Option<SharedString>,
    /// What the diagrams show and for which combination.
    diagram_legend: Option<SharedString>,
    /// Nodes the draw tool has taken, ringed in the accent colour.
    picked: Vec<Point<Pixels>>,
    /// The node under the pointer while a draw tool is active.
    hover_node: Option<Point<Pixels>>,
    /// Where the line from the last picked node is heading.
    rubber: Option<Point<Pixels>>,
    /// Where a click would make a node: the Node tool's, or a Frame or
    /// Shell corner at a snap point.
    ghost: Option<Point<Pixels>>,
    /// The object snap in reach, marked on the geometry it belongs to.
    snap_mark: Option<(snap::Kind, Point<Pixels>)>,
}

fn to_point(x: f64, y: f64) -> Point<Pixels> {
    point(px(x as f32), px(y as f32))
}

impl Viewport {
    fn build_scene(&mut self, bounds: Bounds<Pixels>, cx: &App) -> Scene {
        let document = self.document.read(cx);
        let model = document.model();
        // One classification decides painting, labels, picking, snapping,
        // deformed shapes, diagrams, and fit alike.
        let shown = self.shown(model);
        let context: Vec<Membership> = match (self.mode, self.active_level) {
            (ViewMode::LevelContext, Some(level)) => {
                let (below, above) = levels::neighbours(model, level);
                below
                    .into_iter()
                    .chain(above)
                    .map(|l| levels::membership(model, l))
                    .collect()
            }
            _ => vec![],
        };
        let node_shown = |id: &EntityId| shown.as_ref().is_none_or(|m| m.nodes.contains(id));
        let frame_shown = |id: &EntityId| shown.as_ref().is_none_or(|m| m.frames.contains(id));
        let shell_shown = |id: &EntityId| shown.as_ref().is_none_or(|m| m.shells.contains(id));
        let positions: std::collections::BTreeMap<EntityId, [f64; 3]> = model
            .nodes
            .iter()
            .map(|(id, n)| (*id, n.position.map(|v| v.si())))
            .collect();
        // Underlays on the levels in view, as world segments on their datum.
        let (mode, active_level) = (self.mode, self.active_level);
        let hidden = self.options.hide_underlays;
        let underlay_segments = || {
            model
                .underlays
                .values()
                .filter(move |u| {
                    !hidden && (mode == ViewMode::Whole || Some(u.level) == active_level)
                })
                .filter_map(|u| Some((u, model.levels.get(&u.level)?.elevation.si())))
                .flat_map(|(u, z)| {
                    u.segments.iter().map(move |s| {
                        s.map(|p| [u.origin[0].si() + p[0].si(), u.origin[1].si() + p[1].si(), z])
                    })
                })
        };
        let width = f64::from(bounds.size.width);
        let height = f64::from(bounds.size.height);
        if self.fit_on_next_paint && width > 0.0 && height > 0.0 {
            self.fit_on_next_paint = false;
            self.camera.fit(
                positions
                    .iter()
                    .filter(|(id, _)| node_shown(id))
                    .map(|(_, p)| *p)
                    .chain(underlay_segments().flatten()),
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

        let mut snapshot = Snapshot {
            bounds,
            ..Default::default()
        };
        // Lines are only worth recording while a tool can snap to them.
        let snapping = self.tool != Tool::Select && self.options.snaps.any();
        let mut segments = vec![];
        let mut snappable = |world: [[f64; 3]; 2], screen: [Point<Pixels>; 2]| {
            if snapping {
                segments.push(snap::Segment {
                    world,
                    screen: screen.map(to_f64),
                });
            }
        };
        let nodes = model
            .nodes
            .iter()
            .filter(|(id, _)| node_shown(id))
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
            .filter(|(id, _)| frame_shown(id))
            .filter_map(|(id, frame)| {
                let ends = [
                    *positions.get(&frame.nodes[0])?,
                    *positions.get(&frame.nodes[1])?,
                ];
                let [a, b] = ends.map(|p| project(p).0);
                snapshot.frames.push((*id, a, b));
                snappable(ends, [a, b]);
                Some(Segment {
                    a,
                    b,
                    selected: document.is_selected(*id),
                    label: self.options.frame_labels.then(|| frame.name.clone().into()),
                })
            })
            .collect();
        let quad = |nodes: &[EntityId; 4]| -> Option<([Point<Pixels>; 4], f64)> {
            let mut points = [Point::default(); 4];
            let mut depth = 0.0;
            for (i, n) in nodes.iter().enumerate() {
                let (p, d) = project(*positions.get(n)?);
                points[i] = p;
                depth += d;
            }
            Some((points, depth))
        };
        let mut shells: Vec<Quad> = model
            .shells
            .iter()
            .filter(|(id, _)| shell_shown(id))
            .filter_map(|(id, shell)| {
                let (points, depth) = quad(&shell.nodes)?;
                snapshot.shells.push((*id, points));
                for i in 0..4 {
                    let j = (i + 1) % 4;
                    let edge = [positions[&shell.nodes[i]], positions[&shell.nodes[j]]];
                    snappable(edge, [points[i], points[j]]);
                }
                Some(Quad {
                    points,
                    depth,
                    selected: document.is_selected(*id),
                })
            })
            .collect();
        shells.sort_by(|a, b| a.depth.total_cmp(&b.depth));
        // A drawing can run to many thousands of segments, most of them off
        // screen once zoomed in, so those wholly to one side are dropped.
        let underlays = underlay_segments()
            .map(|world| (world, project(world[0]).0, project(world[1]).0))
            .filter(|(_, a, b)| {
                a.x.max(b.x) >= bounds.left()
                    && a.x.min(b.x) <= bounds.right()
                    && a.y.max(b.y) >= bounds.top()
                    && a.y.min(b.y) <= bounds.bottom()
            })
            .map(|(world, a, b)| {
                snappable(world, [a, b]);
                (a, b)
            })
            .collect();
        snapshot.segments = segments;

        // The storeys beside the active level, and the members spanning to
        // them, as unpickable context.
        let mut context_nodes = vec![];
        let mut context_frames = vec![];
        let mut context_shells = vec![];
        let segment = |nodes: &[EntityId; 2]| -> Option<(Point<Pixels>, Point<Pixels>)> {
            Some((
                project(*positions.get(&nodes[0])?).0,
                project(*positions.get(&nodes[1])?).0,
            ))
        };
        for beside in &context {
            for id in beside.nodes.iter().filter(|id| !node_shown(id)) {
                if let Some(p) = positions.get(id) {
                    context_nodes.push(project(*p).0);
                }
            }
            for id in beside.frames.iter().filter(|id| !frame_shown(id)) {
                if let Some(s) = model.frames.get(id).and_then(|f| segment(&f.nodes)) {
                    context_frames.push(s);
                }
            }
            for id in beside.shells.iter().filter(|id| !shell_shown(id)) {
                if let Some((points, _)) = model.shells.get(id).and_then(|s| quad(&s.nodes)) {
                    context_shells.push(points);
                }
            }
        }
        if self.mode == ViewMode::LevelContext
            && let Some(m) = &shown
        {
            for id in &m.spanning_frames {
                if let Some(s) = model.frames.get(id).and_then(|f| segment(&f.nodes)) {
                    context_frames.push(s);
                }
            }
            for id in &m.spanning_shells {
                if let Some((points, _)) = model.shells.get(id).and_then(|s| quad(&s.nodes)) {
                    context_shells.push(points);
                }
            }
        }
        // In a level view, spanning members show where they meet the plane.
        let mut crossings = vec![];
        let mut traces = vec![];
        if self.mode == ViewMode::Level
            && let (Some(m), Some(level)) = (&shown, self.active_level)
            && let Some(datum) = model.levels.get(&level)
        {
            let elevation = datum.elevation.si();
            for id in &m.spanning_frames {
                let Some(frame) = model.frames.get(id) else {
                    continue;
                };
                let on_level = frame.nodes.iter().find(|n| m.nodes.contains(n));
                let point = match on_level {
                    Some(n) => positions.get(n).copied(),
                    None => match (positions.get(&frame.nodes[0]), positions.get(&frame.nodes[1])) {
                        (Some(a), Some(b)) => plane_crossing(*a, *b, elevation),
                        _ => None,
                    },
                };
                if let Some(p) = point {
                    crossings.push(project(p).0);
                }
            }
            for id in &m.spanning_shells {
                let Some(shell) = model.shells.get(id) else {
                    continue;
                };
                let mut bound = false;
                for i in 0..4 {
                    let (a, b) = (shell.nodes[i], shell.nodes[(i + 1) % 4]);
                    if m.nodes.contains(&a)
                        && m.nodes.contains(&b)
                        && let Some(s) = segment(&[a, b])
                    {
                        traces.push(s);
                        bound = true;
                    }
                }
                // A wall passing through with no edge on this level is cut
                // by the plane instead; no nodes are made for the cut.
                if !bound
                    && let [Some(a), Some(b), Some(c), Some(d)] =
                        shell.nodes.map(|n| positions.get(&n).copied())
                    && let Some((p, q)) = shell_trace([a, b, c, d], elevation)
                {
                    traces.push((project(p).0, project(q).0));
                }
            }
        }
        let level_legend = match (self.mode, self.active_level) {
            (ViewMode::Whole, _) | (_, None) => None,
            (_, Some(level)) => model.levels.get(&level).map(|l| {
                SharedString::from(format!(
                    "{} · {} {}",
                    l.name,
                    fmt_q(Role::Length, l.elevation.si()),
                    UNITS.symbol(Role::Length)
                ))
            }),
        };

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
            for (id, frame) in model.frames.iter().filter(|(id, _)| frame_shown(id)) {
                let _ = id;
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
            let shown_diagrams = analysis.shown_diagrams();
            let column = which.index();
            let max = peak(shown_diagrams, column);
            let extent = model_extent(positions.values().copied());
            let factor = if max > 0.0 { 0.08 * extent / max } else { 0.0 };
            diagram_legend = Some(SharedString::from(format!(
                "{} ({}): {}",
                which.label(),
                UNITS.symbol(which.role()),
                result.combination
            )));
            for (id, frame) in model.frames.iter().filter(|(id, _)| frame_shown(id)) {
                let Some(diagram) = analysis
                    .compiled
                    .mapping
                    .frame_index
                    .get(id)
                    .and_then(|ix| shown_diagrams.get(*ix))
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

        // Draw-tool feedback, aimed with this frame's geometry.
        *self.snapshot.borrow_mut() = snapshot;
        let picked: Vec<Point<Pixels>> = self
            .picked
            .iter()
            .filter_map(|pick| match pick {
                Pick::Node(id) => positions.get(id).copied(),
                Pick::Point { position, .. } => Some(*position),
            })
            .map(|p| project(p).0)
            .collect();
        let hover_node = match self.tool {
            Tool::Select => None,
            _ => self
                .hover_node
                .and_then(|id| positions.get(&id).map(|p| project(*p).0)),
        };
        let aim = match (self.tool, self.hover_node, self.hover) {
            (Tool::Select, ..) => None,
            (_, None, Some(hover)) => self.aim(hover, model),
            _ => None,
        };
        let ghost = aim.as_ref().map(|aim| project(aim.position).0);
        let snap_mark = aim
            .and_then(|aim| aim.snap)
            .map(|hit| (hit.kind, to_point(hit.screen.0, hit.screen.1)));
        let rubber = match self.tool {
            Tool::Frame | Tool::Shell if !picked.is_empty() => {
                hover_node.or(ghost).or(self.hover)
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
        Scene {
            bounds,
            nodes,
            frames,
            shells,
            context_nodes,
            context_frames,
            context_shells,
            underlays,
            crossings,
            traces,
            deformed_frames,
            diagrams,
            axes,
            axes_origin,
            level_legend,
            combination,
            diagram_legend,
            picked,
            hover_node,
            rubber,
            ghost,
            snap_mark,
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

/// Segments stroked as one path. The tessellator indexes its vertices with
/// 16 bits and spends four on a segment, so a path holds 16,384 at most, and
/// one that overflows is lost whole: an underlay can run well past that.
const STROKE_BATCH: usize = 8192;

fn stroke_paths(
    segments: impl Iterator<Item = (Point<Pixels>, Point<Pixels>)>,
    width: Pixels,
) -> Vec<Path<Pixels>> {
    let mut paths = vec![];
    let mut builder = PathBuilder::stroke(width);
    let mut count = 0;
    for (a, b) in segments {
        builder.move_to(a);
        builder.line_to(b);
        count += 1;
        if count == STROKE_BATCH {
            paths.extend(std::mem::replace(&mut builder, PathBuilder::stroke(width)).build());
            count = 0;
        }
    }
    if count > 0 {
        paths.extend(builder.build());
    }
    paths
}

pub(crate) fn stroke_segments(
    segments: impl Iterator<Item = (Point<Pixels>, Point<Pixels>)>,
    width: Pixels,
    color: Hsla,
    window: &mut Window,
) {
    for path in stroke_paths(segments, width) {
        window.paint_path(path, color);
    }
}

/// A hollow circle, for nodes the draw tool has taken or is about to take,
/// and for where a spanning member meets the active level.
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

/// The marker for an object snap, in the shapes CAD packages use: a square
/// on an endpoint, a triangle on a midpoint, a cross on an intersection, and
/// a right angle on a perpendicular. The status bar's toggles match.
fn snap_glyph(kind: snap::Kind, centre: Point<Pixels>) -> Vec<(Point<Pixels>, Point<Pixels>)> {
    let at = |x: f32, y: f32| point(centre.x + px(x), centre.y + px(y));
    let outline: &[(f32, f32)] = match kind {
        snap::Kind::Endpoint => &[(-6., -6.), (6., -6.), (6., 6.), (-6., 6.), (-6., -6.)],
        snap::Kind::Midpoint => &[(0., -7.), (7., 5.), (-7., 5.), (0., -7.)],
        snap::Kind::Intersection => {
            return vec![
                (at(-6., -6.), at(6., 6.)),
                (at(6., -6.), at(-6., 6.)),
            ];
        }
        snap::Kind::Perpendicular => {
            return vec![
                (at(-6., -6.), at(-6., 6.)),
                (at(-6., 6.), at(6., 6.)),
                (at(-6., 0.), at(0., 0.)),
                (at(0., 0.), at(0., 6.)),
            ];
        }
    };
    outline
        .windows(2)
        .map(|w| (at(w[0].0, w[0].1), at(w[1].0, w[1].1)))
        .collect()
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
            stroke_segments(
                scene.underlays.iter().copied(),
                px(1.),
                palette.underlay,
                window,
            );
            // Context sits under everything, faint enough not to read as the floor.
            for points in &scene.context_shells {
                let mut builder = PathBuilder::fill();
                builder.add_polygon(points, true);
                if let Ok(path) = builder.build() {
                    window.paint_path(path, palette.context.opacity(0.12));
                }
                stroke_segments(
                    (0..4).map(|i| (points[i], points[(i + 1) % 4])),
                    px(1.),
                    palette.context.opacity(0.5),
                    window,
                );
            }
            stroke_segments(
                scene.context_frames.iter().copied(),
                px(1.),
                palette.context,
                window,
            );
            for p in &scene.context_nodes {
                let bounds = Bounds::centered_at(*p, size(px(4.), px(4.)));
                window.paint_quad(fill(bounds, palette.context));
            }
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
                scene.traces.iter().copied(),
                px(1.5),
                palette.shell.opacity(0.6),
                window,
            );
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
            // Spanning members meet the plane as rings, distinct from the
            // filled squares that are nodes.
            for p in &scene.crossings {
                paint_ring(*p, 10., 1.5, palette.frame, window);
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
            if let Some((kind, p)) = scene.snap_mark {
                stroke_segments(snap_glyph(kind, p).into_iter(), px(2.), palette.snap, window);
                let label = SharedString::new_static(kind.label());
                let origin = point(p.x + px(12.), p.y + px(6.));
                paint_label(&label, origin, palette.snap, &style, window, cx);
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
            if let Some(level) = &scene.level_legend {
                let origin = point(scene.bounds.left() + px(12.), legend_top);
                paint_label(level, origin, palette.label, &style, window, cx);
                legend_top += px(16.);
            }
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
    /// View presets, fit, the up axis, the view mode, and the active level
    /// in the top-right corner, each labelled with its key. Text only; the
    /// key map is the affordance.
    fn view_controls(&self, cx: &App) -> AnyElement {
        let preset = self.preset;
        let up = self.camera.up;
        let mode = self.mode;
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
        let modes = ButtonGroup::new("view-mode")
            .small()
            .outline()
            .child(
                Button::new("mode-whole")
                    .label("All  5")
                    .selected(mode == ViewMode::Whole)
                    .tooltip_with_action("Show the whole model", &ViewWholeModel, None),
            )
            .child(
                Button::new("mode-level")
                    .label("Level  6")
                    .selected(mode == ViewMode::Level)
                    .tooltip_with_action("Show the active level's floor", &ViewActiveLevel, None),
            )
            .child(
                Button::new("mode-context")
                    .label("Context  7")
                    .selected(mode == ViewMode::LevelContext)
                    .tooltip_with_action(
                        "Show the active level with the storeys beside it",
                        &ViewActiveLevelContext,
                        None,
                    ),
            )
            .on_click(|clicks, window, cx| {
                let action: Box<dyn Action> = match clicks.first() {
                    Some(0) => Box::new(ViewWholeModel),
                    Some(1) => Box::new(ViewActiveLevel),
                    _ => Box::new(ViewActiveLevelContext),
                };
                window.dispatch_action(action, cx);
            });
        let model = self.document.read(cx).model();
        let active = self.active_level;
        let levels: Vec<(EntityId, SharedString)> = model
            .levels_by_elevation()
            .into_iter()
            .map(|id| {
                let name = model.name_of(id).unwrap_or("?").to_string();
                (id, SharedString::from(name))
            })
            .collect();
        let level_name: SharedString = active
            .and_then(|l| model.levels.get(&l))
            .map(|l| l.name.clone())
            .unwrap_or_else(|| "No level".into())
            .into();
        let level = h_flex()
            .gap_1()
            .items_center()
            .child(
                Button::new("level-down")
                    .small()
                    .outline()
                    .compact()
                    .icon(IconName::ArrowDown)
                    .tooltip_with_action("Make the level below active", &LevelDown, None)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(LevelDown), cx)),
            )
            .child(
                Button::new("level-up")
                    .small()
                    .outline()
                    .compact()
                    .icon(IconName::ArrowUp)
                    .tooltip_with_action("Make the level above active", &LevelUp, None)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(LevelUp), cx)),
            )
            .child(
                Button::new("level-select")
                    .small()
                    .outline()
                    .w(px(128.))
                    .label(level_name)
                    .dropdown_caret(true)
                    .tooltip("The active level: pick another from the list")
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                        levels.iter().fold(menu, |menu, (id, label)| {
                            let id = *id;
                            menu.item(
                                PopupMenuItem::new(label.clone())
                                    .checked(active == Some(id))
                                    .on_click(move |_, window, cx| {
                                        window.dispatch_action(Box::new(SetActiveLevel(id.0)), cx)
                                    }),
                            )
                        })
                    }),
            );
        v_flex()
            .absolute()
            .top_4()
            .right_4()
            .gap_2()
            .items_end()
            .occlude()
            .child(h_flex().gap_2().child(camera).child(fit).child(up_axis))
            .child(h_flex().gap_2().child(modes).child(level))
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
                        .child(step("1  Define the levels, a material, and a section", "Ctrl+M · Ctrl+T"))
                        .child(step("2  Draw nodes on the active level, then frames and shells", "N · F · S"))
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
            context: theme.muted_foreground.opacity(0.45),
            underlay: theme.chart_4.opacity(0.6),
            snap: theme.success,
            axes: [theme.red, theme.green, theme.blue],
        };
        let background = theme.background;
        let hint_color = theme.muted_foreground;
        let empty = {
            let model = self.document.read(cx).model();
            model.nodes.is_empty() && model.underlays.is_empty()
        };
        let card = empty.then(|| start_card(theme));
        let controls = self.view_controls(cx);
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

#[cfg(test)]
mod tests {
    // Named imports: the gpui glob carries its own `test` attribute macro.
    use super::{plane_crossing, shell_trace, stroke_paths, to_point};
    use gpui_kit::px;

    #[test]
    fn a_wall_through_an_intermediate_level_leaves_a_trace() {
        // Base to roof, 0 to 8 m, cut at a 4 m level none of its nodes bind to.
        let wall = [
            [0.0, 0.0, 0.0],
            [6.0, 0.0, 0.0],
            [6.0, 0.0, 8.0],
            [0.0, 0.0, 8.0],
        ];
        let (p, q) = shell_trace(wall, 4.0).unwrap();
        let mut xs = [p[0], q[0]];
        xs.sort_by(f64::total_cmp);
        assert_eq!(xs, [0.0, 6.0]);
        assert!(p[2] == 4.0 && q[2] == 4.0);
        assert!(shell_trace(wall, 9.0).is_none(), "the plane misses the wall");
        // A plane through one corner of a tilted panel touches it at a point.
        let tilted = [
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 2.0],
            [4.0, 0.0, 6.0],
            [0.0, 0.0, 4.0],
        ];
        assert!(shell_trace(tilted, 0.0).is_none());
    }

    #[test]
    fn crossing_interpolates_only_between_the_ends() {
        let a = [0.0, 0.0, 0.0];
        let b = [4.0, 2.0, 8.0];
        let p = plane_crossing(a, b, 2.0).unwrap();
        assert!((p[0] - 1.0).abs() < 1e-12 && (p[1] - 0.5).abs() < 1e-12);
        assert_eq!(p[2], 2.0, "the plane's elevation, exactly");
        assert!(plane_crossing(a, b, 9.0).is_none());
        assert!(plane_crossing(a, b, -1.0).is_none());
        assert!(plane_crossing(a, [4.0, 2.0, 0.0], 0.0).is_none(), "a level member has no crossing");
    }

    /// One path cannot hold a whole drawing, and a path that overflows draws
    /// nothing, so a large underlay goes out as several.
    #[test]
    fn a_large_underlay_is_stroked_in_batches_that_all_build() {
        let segments = (0..20_000).map(|i| {
            let x = f64::from(i % 1000);
            (to_point(x, 0.0), to_point(x, 900.0))
        });
        assert_eq!(stroke_paths(segments, px(1.)).len(), 3);
        assert!(stroke_paths(std::iter::empty(), px(1.)).is_empty());
    }
}
