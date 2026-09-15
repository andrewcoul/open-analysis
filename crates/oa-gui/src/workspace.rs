//! The main window: menu bar, ribbon, model tree, prompt strip and 3D view,
//! property panel, and status bar. Every command arrives here as an action,
//! and shapes finished with the draw tools arrive as viewport events.
use crate::actions::*;
use crate::camera::{UpAxis, ViewPreset};
use crate::document::{Document, example_frame, read_model, unused_name, write_model};
use crate::explorer::Explorer;
use crate::properties::PropertyEditor;
use crate::ribbon::{
    AnalysisSummary, Gates, RibbonState, render_prompt, render_ribbon, selection_summary,
};
use crate::viewport::{Tool, Viewport, ViewportEvent};
use gpui_kit::component::command::{
    Command as CommandPalette, CommandGroup, CommandItem, CommandState,
};
use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, GlobalState, Root, TitleBar, WindowExt as _, h_flex,
    h_resizable, resizable_panel, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use oa_core::units::Length;
use oa_model::{
    Axis, Combination, Command, Diaphragm, EntityId, EntityKind, Frame, Group, LoadCase, Model,
    Node, Shell,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct Workspace {
    document: Entity<Document>,
    viewport: Entity<Viewport>,
    explorer: Entity<Explorer>,
    properties: Entity<PropertyEditor>,
    menu_bar: Entity<AppMenuBar>,
    /// The command palette's search state, made on first use.
    palette: Option<Entity<CommandState>>,
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
            cx.subscribe_in(&viewport, window, |this, _, event, window, cx| {
                this.on_viewport_event(event, window, cx)
            }),
        ];
        let mut this = Self {
            document,
            viewport,
            explorer,
            properties,
            menu_bar,
            palette: None,
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
        let preset = viewport.preset();
        let tool = viewport.tool();
        let gates = Gates::of(document);
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
                    MenuItem::action("3D", ViewThreeD).checked(preset == Some(ViewPreset::ThreeD)),
                    MenuItem::action("Plan", ViewPlan).checked(preset == Some(ViewPreset::Plan)),
                    MenuItem::action("Elevation, X across", ViewElevationX)
                        .checked(preset == Some(ViewPreset::ElevationX)),
                    MenuItem::action("Elevation, Y across", ViewElevationY)
                        .checked(preset == Some(ViewPreset::ElevationY)),
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
                    MenuItem::action("Group from selection", AddGroupFromSelection)
                        .disabled(gates.group.is_some()),
                    MenuItem::action("Diaphragm from selected nodes", AddDiaphragmFromSelection)
                        .disabled(gates.diaphragm.is_some()),
                ],
                disabled: false,
            },
            Menu {
                name: "Draw".into(),
                items: vec![
                    MenuItem::action("Select tool", SelectTool).checked(tool == Tool::Select),
                    MenuItem::action("Node tool", NodeTool).checked(tool == Tool::Node),
                    MenuItem::action("Frame tool", FrameTool)
                        .checked(tool == Tool::Frame)
                        .disabled(gates.frame_tool.is_some()),
                    MenuItem::action("Shell tool", ShellTool)
                        .checked(tool == Tool::Shell)
                        .disabled(gates.shell_tool.is_some()),
                    MenuItem::separator(),
                    MenuItem::action("Add node by coordinates…", AddNode),
                    MenuItem::action("Frame between two selected nodes", AddFrameBetweenSelected)
                        .disabled(gates.frame.is_some()),
                    MenuItem::action("Shell from four selected nodes", AddShellFromSelected)
                        .disabled(gates.shell.is_some()),
                ],
                disabled: false,
            },
            Menu {
                name: "Assign".into(),
                items: vec![
                    MenuItem::action("Nodal load to selected nodes…", AddNodalLoad)
                        .disabled(gates.nodal_load.is_some()),
                    MenuItem::action("Uniform load to selected frames…", AddDistributedLoad)
                        .disabled(gates.distributed_load.is_some()),
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
                items: vec![
                    MenuItem::action("Search commands…", OpenCommandPalette),
                    MenuItem::action("Guide and shortcuts…", About),
                ],
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
        self.add_frame([*a, *b], window, cx);
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
        self.add_shell([*n0, *n1, *n2, *n3], window, cx);
    }
    /// A frame from I to J with the first material and section.
    fn add_frame(&mut self, nodes: [EntityId; 2], window: &mut Window, cx: &mut Context<Self>) {
        let (material, section) = match self.first_material_and_section(cx) {
            Ok(x) => x,
            Err(e) => return self.error(e, window, cx),
        };
        let model = self.document.read(cx).model();
        let frame = Frame::new(unused_name::<Frame>(model, "F"), nodes, material, section);
        let id = EntityId(model.next_id);
        if self.apply(Command::AddFrame { id, frame }, window, cx) {
            self.select(vec![id], cx);
        }
    }
    /// A 200 mm shell on four nodes with the first material.
    fn add_shell(&mut self, nodes: [EntityId; 4], window: &mut Window, cx: &mut Context<Self>) {
        let material = match self.first_material_and_section(cx) {
            Ok((material, _)) => material,
            Err(e) => return self.error(e, window, cx),
        };
        let model = self.document.read(cx).model();
        let shell = Shell {
            name: unused_name::<Shell>(model, "SH"),
            nodes,
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
    /// A node at a point the Node tool clicked.
    fn place_node(&mut self, position: [f64; 3], window: &mut Window, cx: &mut Context<Self>) {
        let model = self.document.read(cx).model();
        let node = Node::new(
            unused_name::<Node>(model, "N"),
            position.map(Length::from_metres),
        );
        let id = EntityId(model.next_id);
        if self.apply(Command::AddNode { id, node }, window, cx) {
            self.select(vec![id], cx);
        }
    }

    // MARK: Tools

    fn on_viewport_event(
        &mut self,
        event: &ViewportEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ViewportEvent::PlaceNode(position) => self.place_node(*position, window, cx),
            ViewportEvent::DrawFrame(nodes) => self.add_frame(*nodes, window, cx),
            ViewportEvent::DrawShell(nodes) => self.add_shell(*nodes, window, cx),
        }
    }
    pub fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.viewport
            .update(cx, |viewport, cx| viewport.set_tool(tool, cx));
    }
    /// With exactly two nodes selected, draws between them; otherwise picks up the tool.
    pub fn use_frame_tool(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nodes = self.document.read(cx).selected_of(EntityKind::Node);
        match nodes.as_slice() {
            [a, b] => self.add_frame([*a, *b], window, cx),
            _ => self.set_tool(Tool::Frame, cx),
        }
    }
    /// With exactly four nodes selected, draws on them; otherwise picks up the tool.
    pub fn use_shell_tool(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nodes = self.document.read(cx).selected_of(EntityKind::Node);
        match nodes.as_slice() {
            [a, b, c, d] => self.add_shell([*a, *b, *c, *d], window, cx),
            _ => self.set_tool(Tool::Shell, cx),
        }
    }
    /// Escape: drops the shape being drawn, then the tool, then the selection.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        let handled = self.viewport.update(cx, |viewport, cx| viewport.cancel(cx));
        if !handled {
            self.deselect_all(cx);
        }
    }

    // MARK: Command palette

    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = match &self.palette {
            Some(state) => state.clone(),
            None => {
                let state = cx.new(|cx| CommandState::new(window, cx));
                self.palette = Some(state.clone());
                state
            }
        };
        state.update(cx, |state, cx| state.set_query("", window, cx));
        let workspace = cx.entity().downgrade();
        let palette_state = state.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let groups = workspace
                .upgrade()
                .map(|workspace| workspace.read(cx).palette_entries(cx))
                .unwrap_or_default();
            let mut palette = CommandPalette::new(&palette_state)
                .placeholder("Type a command, or the name of something to define…")
                .bordered(false)
                .max_h(px(420.))
                .on_confirm(|_, window, cx| window.close_dialog(cx))
                .on_cancel(|window, cx| window.close_dialog(cx));
            for (label, items) in groups {
                palette = palette.group(CommandGroup::new().label(label).items(items));
            }
            dialog.w(px(620.)).footer(div()).child(palette)
        });
        window.defer(cx, move |window, cx| {
            state.update(cx, |state, cx| state.focus(window, cx))
        });
    }

    /// Every action, grouped like the ribbon. Gated ones stay listed, greyed,
    /// with the reason in the label so a search never comes up empty.
    fn palette_entries(&self, cx: &App) -> Vec<(&'static str, Vec<CommandItem>)> {
        let document = self.document.read(cx);
        let viewport = self.viewport.read(cx);
        let gates = Gates::of(document);
        let options = viewport.options();
        let solved = document.analysis().is_some();
        fn item(label: &str, action: Box<dyn Action>, gate: Option<&str>) -> CommandItem {
            let item = CommandItem::new().action(action);
            match gate {
                None => item.label(label.to_string()),
                Some(reason) => item.label(format!("{label} — {reason}")).disabled(true),
            }
        }
        let on = |label: &str, checked: bool| {
            if checked {
                format!("{label} (on)")
            } else {
                label.to_string()
            }
        };
        let mut analyze = vec![item("Run static analysis", Box::new(RunStaticAnalysis), None)];
        if let Some(analysis) = document.analysis() {
            for c in &analysis.results.combinations {
                analyze.push(item(
                    &format!("Show combination: {}", c.combination),
                    Box::new(ShowCombination(c.combination.clone().into())),
                    None,
                ));
            }
        }
        vec![
            (
                "File",
                vec![
                    item("New model", Box::new(NewModel), None),
                    item("New example frame", Box::new(NewExampleModel), None),
                    item("Open…", Box::new(OpenModel), None),
                    item("Save", Box::new(SaveModel), None),
                    item("Save as…", Box::new(SaveModelAs), None),
                ],
            ),
            (
                "Edit",
                vec![
                    item("Undo", Box::new(Undo), (!document.can_undo()).then_some("nothing to undo")),
                    item("Redo", Box::new(Redo), (!document.can_redo()).then_some("nothing to redo")),
                    item("Select all", Box::new(SelectAll), None),
                    item("Deselect", Box::new(DeselectAll), None),
                    item(
                        "Delete selected",
                        Box::new(DeleteSelected),
                        document.selection().is_empty().then_some("select something first"),
                    ),
                ],
            ),
            (
                "Draw",
                vec![
                    item("Select tool", Box::new(SelectTool), None),
                    item("Node tool", Box::new(NodeTool), None),
                    item("Frame tool", Box::new(FrameTool), gates.frame_tool),
                    item("Shell tool", Box::new(ShellTool), gates.shell_tool),
                    item("Add node by coordinates…", Box::new(AddNode), None),
                    item(
                        "Frame between two selected nodes",
                        Box::new(AddFrameBetweenSelected),
                        gates.frame,
                    ),
                    item(
                        "Shell from four selected nodes",
                        Box::new(AddShellFromSelected),
                        gates.shell,
                    ),
                ],
            ),
            (
                "Define",
                vec![
                    item("Material from library…", Box::new(AddMaterialFromLibrary), None),
                    item("Custom material…", Box::new(AddCustomMaterial), None),
                    item("Section from library…", Box::new(AddSectionFromLibrary), None),
                    item("Custom section…", Box::new(AddCustomSection), None),
                    item("Add load case", Box::new(AddLoadCase), None),
                    item("Add load combination", Box::new(AddCombination), None),
                ],
            ),
            (
                "Assign to selection",
                vec![
                    item("Nodal load…", Box::new(AddNodalLoad), gates.nodal_load),
                    item("Uniform load…", Box::new(AddDistributedLoad), gates.distributed_load),
                    item("Group from selection", Box::new(AddGroupFromSelection), gates.group),
                    item(
                        "Diaphragm from selected nodes",
                        Box::new(AddDiaphragmFromSelection),
                        gates.diaphragm,
                    ),
                ],
            ),
            (
                "View",
                vec![
                    item("3D view", Box::new(ViewThreeD), None),
                    item("Plan view", Box::new(ViewPlan), None),
                    item("Elevation, X across", Box::new(ViewElevationX), None),
                    item("Elevation, Y across", Box::new(ViewElevationY), None),
                    item("Zoom extents", Box::new(ZoomExtents), None),
                    item(&on("Node labels", options.node_labels), Box::new(ToggleNodeLabels), None),
                    item(&on("Frame labels", options.frame_labels), Box::new(ToggleFrameLabels), None),
                    item(
                        match viewport.up_axis() {
                            UpAxis::Y => "Draw Z as up",
                            UpAxis::Z => "Draw Y as up",
                        },
                        Box::new(ToggleUpAxis),
                        None,
                    ),
                    item(
                        &on("Deformed shape", options.deformed),
                        Box::new(ToggleDeformedShape),
                        (!solved).then_some("run the analysis first"),
                    ),
                ],
            ),
            ("Analyze", analyze),
            ("Help", vec![item("Guide and shortcuts…", Box::new(About), None)]),
        ]
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
        window.open_dialog(cx, |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let heading = move |text: &'static str| {
                div()
                    .mt_2()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child(text)
            };
            let row = move |keys: &'static str, what: &'static str| {
                h_flex()
                    .gap_3()
                    .child(div().w(px(150.)).text_color(muted).child(keys))
                    .child(what)
            };
            dialog
                .title("Guide")
                .w(px(480.))
                .child(
                    v_flex()
                        .gap_1()
                        .text_sm()
                        .child("The ribbon reads left to right in the order a model is built: define a material and section, draw nodes and frames, assign loads, run. The strip above the view says what the current tool wants next. Select things in the view or the model tree and edit them in the panel on the right.")
                        .child("Models are SI: metres, newtons, pascals, kilograms.")
                        .child(heading("Mouse"))
                        .child(row("Click", "Select; shift+click adds to the selection"))
                        .child(row("Right-drag", "Orbit"))
                        .child(row("Middle-drag", "Pan (or shift + right-drag)"))
                        .child(row("Wheel", "Zoom about the pointer"))
                        .child(heading("Keyboard"))
                        .child(row("Ctrl+N / Ctrl+O", "New model / Open"))
                        .child(row("Ctrl+S / Ctrl+Shift+S", "Save / Save as"))
                        .child(row("Ctrl+Z / Ctrl+Y", "Undo / Redo"))
                        .child(row("Ctrl+A", "Select all"))
                        .child(row("Esc", "Stop drawing, then back to Select, then deselect"))
                        .child(row("Delete", "Delete the selection"))
                        .child(row("Ctrl+K", "Search every command"))
                        .child(row("Ctrl+1 to Ctrl+4", "3D, plan, elevation X, elevation Y"))
                        .child(row("F2", "Zoom extents"))
                        .child(row("F5", "Run static analysis"))
                        .child(heading("Drawing"))
                        .child("Node places a node where you click, on the ground plane. Frame joins node I to node J and carries on from J. Shell takes four nodes in order around it. With nodes already selected, Frame and Shell draw on them at once. Loads go on the selected nodes or frames."),
                )
        });
    }

    // MARK: Rendering

    fn ribbon_state(&self, cx: &App) -> RibbonState {
        let document = self.document.read(cx);
        let viewport = self.viewport.read(cx);
        let model = document.model();
        RibbonState {
            can_undo: document.can_undo(),
            can_redo: document.can_redo(),
            has_selection: !document.selection().is_empty(),
            selection: selection_summary(document),
            gates: Gates::of(document),
            options: viewport.options(),
            tool: viewport.tool(),
            picked: viewport
                .picked()
                .iter()
                .filter_map(|id| model.nodes.get(id).map(|n| n.name.clone()))
                .collect(),
            results: document.results_state(),
            analysis: document.analysis().map(|analysis| AnalysisSummary {
                combinations: analysis
                    .results
                    .combinations
                    .iter()
                    .map(|c| SharedString::from(c.combination.clone()))
                    .collect(),
                shown: analysis.combination,
            }),
            empty: model.nodes.is_empty(),
            defaults: model
                .materials
                .values()
                .next()
                .zip(model.sections.values().next())
                .map(|(material, section)| (material.name.clone(), section.name.clone())),
            in_elevation: matches!(
                viewport.preset(),
                Some(ViewPreset::ElevationX | ViewPreset::ElevationY)
            ),
        }
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let document = self.document.read(cx);
        let viewport = self.viewport.read(cx);
        let model = document.model();
        let problems = document.problems();
        let theme = cx.theme();
        let (warning, muted, success) = (theme.warning, theme.muted_foreground, theme.success);
        let problem_text = match problems.len() {
            0 => "Model is valid".to_string(),
            1 => problems[0].to_string(),
            n => format!("{n} problems: {}", problems[0]),
        };
        let valid = problems.is_empty();
        let view = match viewport.preset() {
            Some(ViewPreset::ThreeD) => "3D view",
            Some(ViewPreset::Plan) => "Plan view",
            Some(ViewPreset::ElevationX) => "Elevation, X across",
            Some(ViewPreset::ElevationY) => "Elevation, Y across",
            None => "Free orbit",
        };
        StatusBar::new()
            .left(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .text_xs()
                    .child(
                        div()
                            .size(px(7.))
                            .rounded_full()
                            .bg(if valid { success } else { warning }),
                    )
                    .child(div().when(!valid, |d| d.text_color(warning)).child(problem_text)),
            )
            .left(div().text_xs().text_color(muted).child(format!(
                "{} nodes · {} frames · {} shells · {} cases · {} combinations",
                model.nodes.len(),
                model.frames.len(),
                model.shells.len(),
                model.load_cases.len(),
                model.combinations.len()
            )))
            .right(div().text_xs().text_color(muted).child("SI: m, N, Pa"))
            .right(div().text_xs().text_color(muted).child(
                match viewport.up_axis() {
                    UpAxis::Y => "Y up",
                    UpAxis::Z => "Z up",
                },
            ))
            .right(div().text_xs().text_color(muted).child(view))
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
            .child(render_ribbon(self.ribbon_state(cx), cx))
            .child(
                div().flex_1().min_h_0().w_full().child(
                    h_resizable("main-panels")
                        .child(
                            resizable_panel()
                                .size(px(240.))
                                .size_range(px(160.)..px(600.))
                                .child(self.explorer.clone()),
                        )
                        .child(
                            resizable_panel().child(
                                v_flex()
                                    .size_full()
                                    .child(render_prompt(&self.ribbon_state(cx), cx))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .w_full()
                                            .child(self.viewport.clone()),
                                    ),
                            ),
                        )
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
