//! The main window: menu bar, toolbar, model tree, 3D view, property panel,
//! and status bar. Every command arrives here as an action.
use crate::actions::*;
use crate::camera::{UpAxis, ViewPreset};
use crate::document::{Document, example_frame, read_model, unused_name, write_model};
use crate::explorer::Explorer;
use crate::properties::PropertyEditor;
use crate::viewport::Viewport;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, GlobalState, IconName, Root, Selectable as _, Sizable as _,
    TitleBar, WindowExt as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use oa_core::units::Length;
use oa_model::{
    Axis, Combination, Command, Diaphragm, EntityId, EntityKind, Frame, Group, LoadCase, Model,
    Shell,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct Workspace {
    document: Entity<Document>,
    viewport: Entity<Viewport>,
    explorer: Entity<Explorer>,
    properties: Entity<PropertyEditor>,
    menu_bar: Entity<AppMenuBar>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let document = cx.new(|_| Document::with_model(example_frame(), None));
        let viewport = cx.new(|cx| Viewport::new(document.clone(), cx));
        let explorer = cx.new(|cx| Explorer::new(document.clone(), cx));
        let properties = cx.new(|cx| PropertyEditor::new(document.clone(), window, cx));
        let menu_bar = AppMenuBar::new(cx);
        let subscriptions = vec![
            cx.observe_in(&document, window, |this, _, window, cx| {
                window.set_window_title(&format!(
                    "{} - open-analysis",
                    this.document.read(cx).title()
                ));
                this.refresh_menus(cx);
                cx.notify();
            }),
            cx.observe(&viewport, |this, _, cx| {
                this.refresh_menus(cx);
                cx.notify();
            }),
        ];
        let mut this = Self {
            document,
            viewport,
            explorer,
            properties,
            menu_bar,
            _subscriptions: subscriptions,
        };
        window.set_window_title(&format!(
            "{} - open-analysis",
            this.document.read(cx).title()
        ));
        this.refresh_menus(cx);
        this
    }

    pub fn document(&self) -> &Entity<Document> {
        &self.document
    }

    // MARK: Menus

    fn refresh_menus(&mut self, cx: &mut Context<Self>) {
        // `Menu` holds boxed actions and is not `Clone`, so build it twice.
        cx.set_menus(self.build_menus(cx));
        let owned = self
            .build_menus(cx)
            .into_iter()
            .map(|m| m.owned())
            .collect();
        GlobalState::global_mut(cx).set_app_menus(owned);
        self.menu_bar.update(cx, |menu_bar, cx| menu_bar.reload(cx));
    }

    fn build_menus(&self, cx: &App) -> Vec<Menu> {
        let document = self.document.read(cx);
        let viewport = self.viewport.read(cx);
        let options = viewport.options();
        let combinations: Vec<MenuItem> = document
            .analysis()
            .map(|analysis| {
                analysis
                    .results
                    .combinations
                    .iter()
                    .enumerate()
                    .map(|(ix, c)| {
                        MenuItem::action(
                            c.combination.clone(),
                            ShowCombination(c.combination.clone().into()),
                        )
                        .checked(ix == analysis.combination)
                    })
                    .collect()
            })
            .unwrap_or_default();
        vec![
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New", NewModel),
                    MenuItem::action("New example frame", NewExampleModel),
                    MenuItem::separator(),
                    MenuItem::action("Open…", OpenModel),
                    MenuItem::action("Save", SaveModel),
                    MenuItem::action("Save as…", SaveModelAs),
                    MenuItem::separator(),
                    MenuItem::action("Quit", Quit),
                ],
                disabled: false,
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::action("Undo", Undo).disabled(!document.can_undo()),
                    MenuItem::action("Redo", Redo).disabled(!document.can_redo()),
                    MenuItem::separator(),
                    MenuItem::action("Select all", SelectAll),
                    MenuItem::action("Deselect", DeselectAll),
                    MenuItem::separator(),
                    MenuItem::action("Delete selected", DeleteSelected)
                        .disabled(document.selection().is_empty()),
                ],
                disabled: false,
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("3D", ViewThreeD),
                    MenuItem::action("Plan", ViewPlan),
                    MenuItem::action("Elevation, X across", ViewElevationX),
                    MenuItem::action("Elevation, Y across", ViewElevationY),
                    MenuItem::action("Zoom extents", ZoomExtents),
                    MenuItem::separator(),
                    MenuItem::action("Node labels", ToggleNodeLabels).checked(options.node_labels),
                    MenuItem::action("Frame labels", ToggleFrameLabels)
                        .checked(options.frame_labels),
                    MenuItem::action("Z is up", ToggleUpAxis)
                        .checked(viewport.up_axis() == UpAxis::Z),
                    MenuItem::separator(),
                    MenuItem::action("Deformed shape", ToggleDeformedShape)
                        .checked(options.deformed)
                        .disabled(document.analysis().is_none()),
                ],
                disabled: false,
            },
            Menu {
                name: "Define".into(),
                items: vec![
                    MenuItem::action("Material from library…", AddMaterialFromLibrary),
                    MenuItem::action("Custom material…", AddCustomMaterial),
                    MenuItem::action("Section from library…", AddSectionFromLibrary),
                    MenuItem::action("Custom section…", AddCustomSection),
                    MenuItem::separator(),
                    MenuItem::action("Load case", AddLoadCase),
                    MenuItem::action("Load combination", AddCombination),
                    MenuItem::separator(),
                    MenuItem::action("Group from selection", AddGroupFromSelection),
                    MenuItem::action("Diaphragm from selected nodes", AddDiaphragmFromSelection),
                ],
                disabled: false,
            },
            Menu {
                name: "Draw".into(),
                items: vec![
                    MenuItem::action("Add node…", AddNode),
                    MenuItem::action("Frame between two selected nodes", AddFrameBetweenSelected),
                    MenuItem::action("Shell from four selected nodes", AddShellFromSelected),
                ],
                disabled: false,
            },
            Menu {
                name: "Assign".into(),
                items: vec![
                    MenuItem::action("Nodal load to selected nodes…", AddNodalLoad),
                    MenuItem::action("Uniform load to selected frames…", AddDistributedLoad),
                ],
                disabled: false,
            },
            Menu {
                name: "Analyze".into(),
                items: vec![
                    MenuItem::action("Run static analysis", RunStaticAnalysis),
                    MenuItem::separator(),
                    MenuItem::submenu(Menu {
                        name: "Show combination".into(),
                        disabled: combinations.is_empty(),
                        items: combinations,
                    }),
                ],
                disabled: false,
            },
            Menu {
                name: "Help".into(),
                items: vec![MenuItem::action("About open-analysis", About)],
                disabled: false,
            },
        ]
    }

    // MARK: Feedback

    fn error(&self, message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
        window.push_notification(Notification::error(message), cx);
    }
    fn info(&self, message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
        window.push_notification(Notification::info(message), cx);
    }
    fn apply(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self
            .document
            .update(cx, |document, cx| document.apply(command, cx))
        {
            Ok(()) => true,
            Err(e) => {
                self.error(e.to_string(), window, cx);
                false
            }
        }
    }
    fn select(&mut self, ids: Vec<EntityId>, cx: &mut Context<Self>) {
        self.document
            .update(cx, |document, cx| document.set_selection(ids, cx));
    }

    /// Runs `then` at once, or after the user agrees to drop unsaved changes.
    fn confirm_discard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    ) {
        if !self.document.read(cx).is_dirty() {
            then(self, window, cx);
            return;
        }
        let workspace = cx.entity();
        let then = std::rc::Rc::new(then);
        window.open_alert_dialog(cx, move |alert, _, _| {
            let workspace = workspace.clone();
            let then = then.clone();
            alert
                .title("Discard unsaved changes?")
                .description("The model has changes that have not been saved.")
                .confirm()
                .on_ok(move |_, window, cx| {
                    workspace.update(cx, |this, cx| then(this, window, cx));
                    true
                })
        });
    }

    // MARK: File

    pub fn new_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, |this, _, cx| {
            this.replace(Model::default(), None, cx)
        });
    }
    pub fn new_example(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, |this, _, cx| {
            this.replace(example_frame(), None, cx)
        });
    }
    fn replace(&mut self, model: Model, path: Option<PathBuf>, cx: &mut Context<Self>) {
        self.document
            .update(cx, |document, cx| document.replace(model, path, cx));
        self.viewport
            .update(cx, |viewport, cx| viewport.zoom_extents(cx));
    }
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, |_, window, cx| {
            let receiver = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Open".into()),
            });
            cx.spawn_in(window, async move |this, cx| {
                let Ok(Ok(Some(paths))) = receiver.await else {
                    return;
                };
                let Some(path) = paths.into_iter().next() else {
                    return;
                };
                this.update_in(cx, |this, window, cx| this.load(&path, window, cx))
                    .ok();
            })
            .detach();
        });
    }
    fn load(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        match read_model(path) {
            Ok(model) => {
                self.replace(model, Some(path.to_path_buf()), cx);
                self.info(format!("Opened {}", path.display()), window, cx);
            }
            Err(e) => self.error(e, window, cx),
        }
    }
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.document.read(cx).path().map(Path::to_path_buf) {
            Some(path) => self.write(path, window, cx),
            None => self.save_as(window, cx),
        }
    }
    pub fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.document.read(cx).path().map(Path::to_path_buf);
        let directory = current
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let suggested = current
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "model.oa.json".into());
        let receiver = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = receiver.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.write(path, window, cx))
                .ok();
        })
        .detach();
    }
    fn write(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let result = write_model(self.document.read(cx).model(), &path);
        match result {
            Ok(()) => {
                self.document
                    .update(cx, |document, cx| document.mark_saved(path.clone(), cx));
                self.info(format!("Saved {}", path.display()), window, cx);
            }
            Err(e) => self.error(e, window, cx),
        }
    }

    // MARK: Edit

    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(e) = self.document.update(cx, |document, cx| document.undo(cx)) {
            self.error(e.to_string(), window, cx);
        }
    }
    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(e) = self.document.update(cx, |document, cx| document.redo(cx)) {
            self.error(e.to_string(), window, cx);
        }
    }
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let ids = {
            let model = self.document.read(cx).model();
            model
                .nodes
                .keys()
                .chain(model.frames.keys())
                .chain(model.shells.keys())
                .copied()
                .collect()
        };
        self.select(ids, cx);
    }
    pub fn deselect_all(&mut self, cx: &mut Context<Self>) {
        self.document
            .update(cx, |document, cx| document.clear_selection(cx));
    }

    /// Deletes the selection. Frames and shells on deleted nodes go too, and
    /// loads or diaphragm entries on anything deleted are dropped first.
    pub fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let commands = {
            let document = self.document.read(cx);
            let model = document.model();
            let mut doomed: BTreeSet<EntityId> = document.selection().iter().copied().collect();
            if doomed.is_empty() {
                return;
            }
            for (id, frame) in &model.frames {
                if frame.nodes.iter().any(|n| doomed.contains(n)) {
                    doomed.insert(*id);
                }
            }
            for (id, shell) in &model.shells {
                if shell.nodes.iter().any(|n| doomed.contains(n)) {
                    doomed.insert(*id);
                }
            }
            let mut commands = vec![];
            for (id, case) in &model.load_cases {
                if doomed.contains(id) {
                    continue;
                }
                let mut trimmed = case.clone();
                trimmed.nodal.retain(|l| !doomed.contains(&l.node));
                trimmed.member.retain(|l| !doomed.contains(&l.member()));
                trimmed.surface.retain(|l| !doomed.contains(&l.shell));
                if trimmed != *case {
                    commands.push(Command::UpdateLoadCase {
                        id: *id,
                        load_case: trimmed,
                    });
                }
            }
            for (id, diaphragm) in &model.diaphragms {
                if doomed.contains(id) {
                    continue;
                }
                let mut trimmed = diaphragm.clone();
                trimmed.nodes.retain(|n| !doomed.contains(n));
                if trimmed.master.is_some_and(|m| doomed.contains(&m)) {
                    trimmed.master = None;
                }
                if trimmed != *diaphragm {
                    commands.push(Command::UpdateDiaphragm {
                        id: *id,
                        diaphragm: trimmed,
                    });
                }
            }
            // Dependents before the things they reference.
            let order = [
                EntityKind::Group,
                EntityKind::Combination,
                EntityKind::LoadCase,
                EntityKind::Diaphragm,
                EntityKind::Shell,
                EntityKind::Frame,
                EntityKind::Section,
                EntityKind::Material,
                EntityKind::Node,
            ];
            for kind in order {
                for id in &doomed {
                    if model.kind_of(*id) != Some(kind) {
                        continue;
                    }
                    let id = *id;
                    commands.push(match kind {
                        EntityKind::Group => Command::RemoveGroup { id },
                        EntityKind::Combination => Command::RemoveCombination { id },
                        EntityKind::LoadCase => Command::RemoveLoadCase { id },
                        EntityKind::Diaphragm => Command::RemoveDiaphragm { id },
                        EntityKind::Shell => Command::RemoveShell { id },
                        EntityKind::Frame => Command::RemoveFrame { id },
                        EntityKind::Section => Command::RemoveSection { id },
                        EntityKind::Material => Command::RemoveMaterial { id },
                        EntityKind::Node => Command::RemoveNode { id },
                    });
                }
            }
            commands
        };
        let count = commands.len();
        if self.apply(Command::Batch { commands }, window, cx) {
            self.info(format!("Deleted {count} entities"), window, cx);
        }
    }

    // MARK: Draw and define

    fn first_material_and_section(&self, cx: &App) -> Result<(EntityId, EntityId), String> {
        let model = self.document.read(cx).model();
        let material = model
            .materials
            .keys()
            .next()
            .copied()
            .ok_or("Define a material first (Define menu)")?;
        let section = model
            .sections
            .keys()
            .next()
            .copied()
            .ok_or("Define a section first (Define menu)")?;
        Ok((material, section))
    }
    pub fn add_frame_between_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nodes = self.document.read(cx).selected_of(EntityKind::Node);
        let [a, b] = nodes.as_slice() else {
            self.error("Select exactly two nodes, in order from I to J", window, cx);
            return;
        };
        let (material, section) = match self.first_material_and_section(cx) {
            Ok(x) => x,
            Err(e) => return self.error(e, window, cx),
        };
        let model = self.document.read(cx).model();
        let frame = Frame::new(
            unused_name::<Frame>(model, "F"),
            [*a, *b],
            material,
            section,
        );
        let id = EntityId(model.next_id);
        if self.apply(Command::AddFrame { id, frame }, window, cx) {
            self.select(vec![id], cx);
        }
    }
    pub fn add_shell_from_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nodes = self.document.read(cx).selected_of(EntityKind::Node);
        let [n0, n1, n2, n3] = nodes.as_slice() else {
            self.error(
                "Select exactly four nodes, going around the shell",
                window,
                cx,
            );
            return;
        };
        let material = match self.first_material_and_section(cx) {
            Ok((material, _)) => material,
            Err(e) => return self.error(e, window, cx),
        };
        let model = self.document.read(cx).model();
        let shell = Shell {
            name: unused_name::<Shell>(model, "SH"),
            nodes: [*n0, *n1, *n2, *n3],
            material,
            thickness: Length::from_metres(0.2),
            formulation: Default::default(),
            drilling_ratio: 1e-3,
        };
        let id = EntityId(model.next_id);
        if self.apply(Command::AddShell { id, shell }, window, cx) {
            self.select(vec![id], cx);
        }
    }
    pub fn add_load_case(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.document.read(cx).model();
        let load_case = LoadCase::new(unused_name::<LoadCase>(model, "LC"));
        let id = EntityId(model.next_id);
        if self.apply(Command::AddLoadCase { id, load_case }, window, cx) {
            self.select(vec![id], cx);
        }
    }
    pub fn add_combination(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.document.read(cx).model();
        let combination = Combination {
            name: unused_name::<Combination>(model, "COMB"),
            terms: model
                .load_cases
                .keys()
                .next()
                .map(|c| (*c, 1.0))
                .into_iter()
                .collect(),
        };
        let id = EntityId(model.next_id);
        if self.apply(Command::AddCombination { id, combination }, window, cx) {
            self.select(vec![id], cx);
        }
    }
    pub fn add_group_from_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.document.read(cx);
        let model = document.model();
        let group = Group {
            name: unused_name::<Group>(model, "GROUP"),
            members: document.selection().iter().copied().collect(),
        };
        let id = EntityId(model.next_id);
        if self.apply(Command::AddGroup { id, group }, window, cx) {
            self.select(vec![id], cx);
        }
    }
    pub fn add_diaphragm_from_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.document.read(cx);
        let nodes = document.selected_of(EntityKind::Node);
        if nodes.len() < 2 {
            return self.error(
                "Select the nodes the diaphragm constrains first",
                window,
                cx,
            );
        }
        let model = document.model();
        let diaphragm = Diaphragm {
            name: unused_name::<Diaphragm>(model, "DIAPH"),
            master: None,
            nodes,
            normal: match self.viewport.read(cx).up_axis() {
                UpAxis::Y => Axis::Y,
                UpAxis::Z => Axis::Z,
            },
        };
        let id = EntityId(model.next_id);
        if self.apply(Command::AddDiaphragm { id, diaphragm }, window, cx) {
            self.select(vec![id], cx);
        }
    }

    // MARK: View and analysis

    pub fn set_view(&mut self, preset: ViewPreset, cx: &mut Context<Self>) {
        self.viewport
            .update(cx, |viewport, cx| viewport.set_preset(preset, cx));
    }
    pub fn zoom_extents(&mut self, cx: &mut Context<Self>) {
        self.viewport
            .update(cx, |viewport, cx| viewport.zoom_extents(cx));
    }
    pub fn toggle_up_axis(&mut self, cx: &mut Context<Self>) {
        self.viewport
            .update(cx, |viewport, cx| viewport.toggle_up_axis(cx));
    }
    pub fn toggle_option(
        &mut self,
        which: fn(&mut crate::viewport::DisplayOptions),
        cx: &mut Context<Self>,
    ) {
        self.viewport.update(cx, |viewport, cx| {
            let mut options = viewport.options();
            which(&mut options);
            viewport.set_options(options, cx);
        });
    }
    pub fn run_static(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self
            .document
            .update(cx, |document, cx| document.run_static(cx))
        {
            Ok(count) => {
                self.toggle_option(|o| o.deformed = true, cx);
                self.info(
                    format!("Static analysis solved {count} combinations"),
                    window,
                    cx,
                );
            }
            Err(e) => self.error(format!("Analysis failed: {e}"), window, cx),
        }
    }
    pub fn show_combination(&mut self, name: &str, cx: &mut Context<Self>) {
        let ix = self.document.read(cx).analysis().and_then(|a| {
            a.results
                .combinations
                .iter()
                .position(|c| c.combination == name)
        });
        if let Some(ix) = ix {
            self.document
                .update(cx, |document, cx| document.set_combination(ix, cx));
            self.toggle_option(|o| o.deformed = true, cx);
        }
    }
    pub fn about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, |dialog, _, _| {
            dialog
                .title("open-analysis")
                .child(
                    v_flex()
                        .gap_2()
                        .text_sm()
                        .child("Structural analysis engine and model editor.")
                        .child("Models are SI: metres, newtons, pascals, kilograms.")
                        .child("Right-drag orbits, shift+right or middle-drag pans, the wheel zooms. Click selects, shift+click extends."),
                )
        });
    }

    // MARK: Rendering

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let document = self.document.read(cx);
        let (can_undo, can_redo, has_selection) = (
            document.can_undo(),
            document.can_redo(),
            !document.selection().is_empty(),
        );
        let options = self.viewport.read(cx).options();
        let border = cx.theme().border;
        fn command(
            id: &'static str,
            label: &'static str,
            action: impl Action + Clone + 'static,
        ) -> Button {
            Button::new(id)
                .small()
                .ghost()
                .label(label)
                .on_click(move |_, window, cx| window.dispatch_action(Box::new(action.clone()), cx))
        }
        h_flex()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(border)
            .flex_wrap()
            .child(command("new", "New", NewModel).icon(IconName::File))
            .child(command("open", "Open", OpenModel).icon(IconName::FolderOpen))
            .child(command("save", "Save", SaveModel))
            .child(
                command("undo", "Undo", Undo)
                    .icon(IconName::Undo2)
                    .disabled(!can_undo),
            )
            .child(
                command("redo", "Redo", Redo)
                    .icon(IconName::Redo2)
                    .disabled(!can_redo),
            )
            .child(
                command("delete", "Delete", DeleteSelected)
                    .icon(IconName::Delete)
                    .disabled(!has_selection),
            )
            .child(command("add-node", "Node", AddNode).icon(IconName::Plus))
            .child(command("add-frame", "Frame", AddFrameBetweenSelected).icon(IconName::Plus))
            .child(command("view-3d", "3D", ViewThreeD))
            .child(command("view-plan", "Plan", ViewPlan))
            .child(command("view-x", "Elev X", ViewElevationX))
            .child(command("view-y", "Elev Y", ViewElevationY))
            .child(command("zoom-extents", "Extents", ZoomExtents).icon(IconName::Maximize))
            .child(
                command("node-labels", "Node labels", ToggleNodeLabels)
                    .selected(options.node_labels)
                    .toggled(options.node_labels),
            )
            .child(
                command("frame-labels", "Frame labels", ToggleFrameLabels)
                    .selected(options.frame_labels)
                    .toggled(options.frame_labels),
            )
            .child(command("run", "Run analysis", RunStaticAnalysis).icon(IconName::Play))
            .child(
                command("deformed", "Deformed", ToggleDeformedShape)
                    .selected(options.deformed)
                    .toggled(options.deformed),
            )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let document = self.document.read(cx);
        let model = document.model();
        let problems = document.problems();
        let (warning, muted) = (cx.theme().warning, cx.theme().muted_foreground);
        let problem_text = match problems.len() {
            0 => "Model is valid".to_string(),
            1 => problems[0].to_string(),
            n => format!("{n} problems: {}", problems[0]),
        };
        let selected = document.selection().len();
        StatusBar::new()
            .left(div().text_xs().child(document.title()))
            .left(div().text_xs().text_color(muted).child(format!(
                "{} nodes, {} frames, {} shells, {} cases, {} combinations",
                model.nodes.len(),
                model.frames.len(),
                model.shells.len(),
                model.load_cases.len(),
                model.combinations.len()
            )))
            .left(
                div()
                    .text_xs()
                    .when(!problems.is_empty(), |d| d.text_color(warning))
                    .child(problem_text),
            )
            .right(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("{selected} selected")),
            )
            .right(div().text_xs().text_color(muted).child(
                match self.viewport.read(cx).up_axis() {
                    UpAxis::Y => "Y up",
                    UpAxis::Z => "Z up",
                },
            ))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, foreground, muted) =
            (theme.background, theme.foreground, theme.muted_foreground);
        let title = self.document.read(cx).title();
        v_flex()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .child(
                TitleBar::new()
                    .child(h_flex().child(self.menu_bar.clone()))
                    .child(
                        div()
                            .px_3()
                            .text_sm()
                            .text_color(muted)
                            .child(format!("{title} - open-analysis")),
                    ),
            )
            .child(self.render_toolbar(cx))
            .child(
                div().flex_1().min_h_0().w_full().child(
                    h_resizable("main-panels")
                        .child(
                            resizable_panel()
                                .size(px(240.))
                                .size_range(px(160.)..px(600.))
                                .child(self.explorer.clone()),
                        )
                        .child(resizable_panel().child(self.viewport.clone()))
                        .child(
                            resizable_panel()
                                .size(px(340.))
                                .size_range(px(220.)..px(700.))
                                .child(self.properties.clone()),
                        ),
                ),
            )
            .child(self.render_status_bar(cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}
