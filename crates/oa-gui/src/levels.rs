//! Levels as an editable table in a dialog from Define > Levels. A row per
//! level, lowest first, with its name, elevation, and the storey height
//! below it. Editing the elevation moves that level with its nodes and
//! holds every other level still; editing the height below moves the level
//! and every level above it, so the storey heights above are kept. A move
//! that carries nodes is previewed before it is applied, and a level with
//! nodes on it is removed only after they are moved to a neighbouring level
//! with their coordinates kept. Every edit is one undoable command.
use crate::actions::SetActiveLevel;
use crate::document::{Document, unused_name};
use crate::text::{UNITS, fmt_q, label, parse_q};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::*;
use oa_core::units::Length;
use oa_model::levels::{self, ElevationScope, MovePlan};
use oa_model::{Command, EntityId, Level, Role};

struct Row {
    id: EntityId,
    name: Entity<InputState>,
    elevation: Entity<InputState>,
    /// Absent for the lowest level, which has no level below.
    height: Option<Entity<InputState>>,
    /// What the fields showed when built. Display text is rounded, so an
    /// untouched field is never parsed back.
    committed: [String; 3],
    /// Nodes bound to the level.
    nodes: usize,
    _subscriptions: Vec<Subscription>,
}

pub struct LevelPanel {
    document: Entity<Document>,
    /// Revision the rows were built for.
    built_for: u64,
    rows: Vec<Row>,
    /// Input to focus after the next rebuild, so Enter keeps the caret in place.
    pending_focus: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl LevelPanel {
    pub fn new(document: Entity<Document>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe_in(&document, window, |this, _, window, cx| {
            this.sync(window, cx)
        });
        let mut this = Self {
            document,
            built_for: u64::MAX,
            rows: vec![],
            pending_focus: None,
            _subscriptions: vec![subscription],
        };
        this.sync(window, cx);
        this
    }

    /// Rebuilds the rows when the model changed, keeping focus on the input
    /// asked for by Enter or an add, else on whichever input holds it now.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let revision = self.document.read(cx).revision();
        if self.built_for == revision {
            return;
        }
        self.built_for = revision;
        let focus = self.pending_focus.take().or_else(|| {
            self.inputs()
                .find(|(_, input)| input.read(cx).focus_handle(cx).is_focused(window))
                .map(|(key, _)| key)
        });
        self.build(window, cx);
        if let Some(key) = focus
            && let Some(input) = self.input_for(&key)
        {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn inputs(&self) -> impl Iterator<Item = (String, &Entity<InputState>)> {
        self.rows.iter().flat_map(|r| {
            [
                Some((format!("name-{}", r.id.0), &r.name)),
                Some((format!("elevation-{}", r.id.0), &r.elevation)),
                r.height.as_ref().map(|h| (format!("height-{}", r.id.0), h)),
            ]
            .into_iter()
            .flatten()
        })
    }
    fn input_for(&self, key: &str) -> Option<Entity<InputState>> {
        self.inputs()
            .find(|(k, _)| k == key)
            .map(|(_, input)| input.clone())
    }

    /// A text input that commits its row on Enter or blur.
    fn text_input(
        &self,
        key: String,
        value: String,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Subscription) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    this.pending_focus = Some(key.clone());
                    this.commit(id, window, cx);
                }
                InputEvent::Blur => this.commit(id, window, cx),
                _ => {}
            },
        );
        (input, subscription)
    }

    fn build(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows: Vec<(EntityId, String, String, Option<String>, usize)> = {
            let model = self.document.read(cx).model();
            model
                .levels_by_elevation()
                .into_iter()
                .map(|id| {
                    let l = &model.levels[&id];
                    let height = levels::height_below(model, id).map(|h| fmt_q(Role::Length, h));
                    let nodes = model.nodes.values().filter(|n| n.level == id).count();
                    (
                        id,
                        l.name.clone(),
                        fmt_q(Role::Length, l.elevation.si()),
                        height,
                        nodes,
                    )
                })
                .collect()
        };
        self.rows = rows
            .into_iter()
            .map(|(id, name, elevation, height, nodes)| {
                let (name_input, s1) =
                    self.text_input(format!("name-{}", id.0), name.clone(), id, window, cx);
                let (elevation_input, s2) = self.text_input(
                    format!("elevation-{}", id.0),
                    elevation.clone(),
                    id,
                    window,
                    cx,
                );
                let mut subscriptions = vec![s1, s2];
                let height_input = height.as_ref().map(|h| {
                    let (input, s) =
                        self.text_input(format!("height-{}", id.0), h.clone(), id, window, cx);
                    subscriptions.push(s);
                    input
                });
                Row {
                    id,
                    name: name_input,
                    elevation: elevation_input,
                    height: height_input,
                    committed: [name, elevation, height.unwrap_or_default()],
                    nodes,
                    _subscriptions: subscriptions,
                }
            })
            .collect();
    }

    // MARK: Editing

    fn error(&self, message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
        window.push_notification(Notification::error(message), cx);
    }
    fn apply(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let result = self
            .document
            .update(cx, |document, cx| document.apply(command, cx));
        if let Err(e) = result {
            self.error(e.to_string(), window, cx);
        }
    }

    /// Reads a row and applies what changed: a rename, an elevation that
    /// moves this level alone, or a height that moves it with the levels
    /// above. One edit per commit; the rebuild refreshes the others.
    fn commit(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.iter().find(|r| r.id == id) else {
            return;
        };
        let name = row.name.read(cx).value().to_string();
        let elevation = row.elevation.read(cx).value().to_string();
        let height = row.height.as_ref().map(|h| h.read(cx).value().to_string());
        let [c_name, c_elevation, c_height] = row.committed.clone();
        if name != c_name {
            let level = {
                let model = self.document.read(cx).model();
                let Some(current) = model.levels.get(&id) else {
                    return;
                };
                Level {
                    name,
                    elevation: current.elevation,
                }
            };
            self.apply(Command::UpdateLevel { id, level }, window, cx);
            return;
        }
        if elevation != c_elevation {
            match parse_q(Role::Length, "Elevation", &elevation) {
                Ok(e) => self.move_level(
                    id,
                    Length::from_si(e),
                    ElevationScope::ThisLevel,
                    window,
                    cx,
                ),
                Err(e) => self.error(e, window, cx),
            }
            return;
        }
        if let Some(height) = height
            && height != c_height
        {
            let target = parse_q(Role::Length, "Height below", &height).and_then(|h| {
                let model = self.document.read(cx).model();
                let (below, _) = levels::neighbours(model, id);
                below
                    .map(|b| model.levels[&b].elevation.si() + h)
                    .ok_or_else(|| "The lowest level has no height below".to_string())
            });
            match target {
                Ok(e) => self.move_level(
                    id,
                    Length::from_si(e),
                    ElevationScope::ThisAndAbove,
                    window,
                    cx,
                ),
                Err(e) => self.error(e, window, cx),
            }
        }
    }

    /// Plans the move and applies it, after a preview of what it carries
    /// when nodes are involved. A refused move is reported and nothing changes.
    fn move_level(
        &mut self,
        id: EntityId,
        elevation: Length,
        scope: ElevationScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plan: Result<(MovePlan, String), String> = {
            let model = self.document.read(cx).model();
            levels::plan_set_elevation(model, id, elevation, scope)
                .map(|plan| {
                    let names: Vec<&str> = model
                        .levels_by_elevation()
                        .into_iter()
                        .filter(|l| plan.levels.contains(l))
                        .filter_map(|l| model.name_of(l))
                        .collect();
                    let members = model
                        .frames
                        .values()
                        .filter(|f| f.nodes.iter().any(|n| plan.nodes.contains(n)))
                        .count()
                        + model
                            .shells
                            .values()
                            .filter(|s| s.nodes.iter().any(|n| plan.nodes.contains(n)))
                            .count();
                    let summary = format!(
                        "Move {} by {} {}, carrying {} nodes and the {} members on them. Other levels and their nodes stay where they are.",
                        names.join(", "),
                        fmt_q(Role::Length, plan.delta),
                        UNITS.symbol(Role::Length),
                        plan.nodes.len(),
                        members,
                    );
                    (plan, summary)
                })
                .map_err(|e| e.to_string())
        };
        let (plan, summary) = match plan {
            Ok(x) => x,
            Err(e) => return self.error(e, window, cx),
        };
        let command = Command::SetLevelElevation {
            id,
            elevation,
            scope,
        };
        if plan.nodes.is_empty() {
            return self.apply(command, window, cx);
        }
        let document = self.document.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let document = document.clone();
            let command = command.clone();
            alert
                .title("Move level?")
                .description(summary.clone())
                .confirm()
                .on_ok(move |_, window, cx| {
                    if let Err(e) =
                        document.update(cx, |document, cx| document.apply(command.clone(), cx))
                    {
                        window.push_notification(Notification::error(e.to_string()), cx);
                    }
                    true
                })
        });
    }

    /// A new level one storey above the top one: the last storey height, or
    /// 12 ft when there is only one level.
    fn add_level(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (id, level) = {
            let model = self.document.read(cx).model();
            let order = model.levels_by_elevation();
            let top = order.last().copied();
            let elevation = top.map_or(0.0, |t| model.levels[&t].elevation.si());
            let storey = top
                .and_then(|t| levels::height_below(model, t))
                .unwrap_or(Length::from_feet(12.0).si());
            (
                EntityId(model.next_id),
                Level::new(
                    unused_name::<Level>(model, "Level "),
                    Length::from_si(elevation + storey),
                ),
            )
        };
        self.pending_focus = Some(format!("name-{}", id.0));
        self.apply(Command::AddLevel { id, level }, window, cx);
    }

    /// Removes a level. Nodes on it first move to the level below (or above,
    /// for the lowest), keeping their coordinates, after a confirmation.
    fn remove_level(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let plan: Result<Option<(Vec<Command>, String)>, String> = {
            let model = self.document.read(cx).model();
            let bound = model.nodes.values().filter(|n| n.level == id).count();
            if bound == 0 {
                Ok(None)
            } else {
                let (below, above) = levels::neighbours(model, id);
                match below.or(above) {
                    None => Err("A model keeps at least one level".to_string()),
                    Some(target) => levels::plan_remove(model, id, target)
                        .map(|commands| {
                            let summary = format!(
                                "Move the {bound} nodes on {} to {}, keeping their positions, and remove the level?",
                                model.name_of(id).unwrap_or("?"),
                                model.name_of(target).unwrap_or("?"),
                            );
                            Some((commands, summary))
                        })
                        .map_err(|e| e.to_string()),
                }
            }
        };
        match plan {
            Err(e) => self.error(e, window, cx),
            Ok(None) => self.apply(Command::RemoveLevel { id }, window, cx),
            Ok(Some((commands, summary))) => {
                let document = self.document.clone();
                window.open_alert_dialog(cx, move |alert, _, _| {
                    let document = document.clone();
                    let commands = commands.clone();
                    alert
                        .title("Remove level?")
                        .description(summary.clone())
                        .confirm()
                        .on_ok(move |_, window, cx| {
                            let command = Command::Batch {
                                commands: commands.clone(),
                            };
                            if let Err(e) =
                                document.update(cx, |document, cx| document.apply(command, cx))
                            {
                                window.push_notification(Notification::error(e.to_string()), cx);
                            }
                            true
                        })
                });
            }
        }
    }

    // MARK: Rendering

    fn render_table(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let header = TableRow::new()
            .child(TableHead::new().child("Name"))
            .child(TableHead::new().child(label("Elevation", Role::Length)))
            .child(TableHead::new().child(label("Height below", Role::Length)))
            .child(TableHead::new().child("Nodes"))
            .child(TableHead::new().child(""));
        let rows = self.rows.iter().enumerate().map(|(ix, row)| {
            let id = row.id;
            let height = match &row.height {
                Some(input) => Input::new(input).small().into_any_element(),
                None => div()
                    .text_xs()
                    .text_color(muted)
                    .child("lowest")
                    .into_any_element(),
            };
            TableRow::new()
                .child(TableCell::new().child(Input::new(&row.name).small()))
                .child(TableCell::new().child(Input::new(&row.elevation).small()))
                .child(TableCell::new().child(height))
                .child(TableCell::new().child(div().text_sm().child(row.nodes.to_string())))
                .child(
                    TableCell::new().child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(("go-to-level", ix))
                                    .xsmall()
                                    .outline()
                                    .label("Go to")
                                    .on_click(move |_, window, cx| {
                                        window.dispatch_action(Box::new(SetActiveLevel(id.0)), cx)
                                    }),
                            )
                            .child(
                                Button::new(("remove-level", ix))
                                    .xsmall()
                                    .danger()
                                    .outline()
                                    .label("Remove")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.remove_level(id, window, cx)
                                    })),
                            ),
                    ),
                )
        });
        Table::new()
            .small()
            .child(TableHeader::new().child(header))
            .child(TableBody::new().children(rows))
            .into_any_element()
    }
}

impl Render for LevelPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .id("levels-scroll")
            .size_full()
            .overflow_scrollbar()
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(muted).child(
                        "Lowest first. Elevation moves a level with its nodes; height below moves it with every level above.",
                    ))
                    .child(self.render_table(cx))
                    .child(
                        h_flex().gap_2().child(
                            Button::new("add-level")
                                .small()
                                .outline()
                                .label("Add level above")
                                .on_click(cx.listener(|this, _, window, cx| this.add_level(window, cx))),
                        ),
                    ),
            )
    }
}
