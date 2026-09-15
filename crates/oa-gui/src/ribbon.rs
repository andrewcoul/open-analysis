//! One ribbon under the title bar, ordered the way a model is built: File,
//! Edit, Draw, Define, Assign, Analyze, each a captioned group. There are no
//! tabs, so every command is visible at once. The Draw group holds the sticky
//! tools, the Analyze group keeps Run in view beside a chip saying whether the
//! results are current, and a search button opens the command palette.
//! [`render_prompt`] draws the strip above the viewport that says what the
//! current tool wants next. Every button carries a tooltip with its shortcut;
//! a command that needs something first is greyed with the tooltip saying
//! what. Menus and the palette share the same [`Gates`].
use crate::actions::*;
use crate::document::{Document, ResultsState};
use crate::viewport::{DisplayOptions, Tool};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Theme, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use oa_model::EntityKind;

/// Why each gated command is unavailable, or `None` when it can run. The
/// ribbon shows the reason in the tooltip; menus and the palette disable the item.
pub struct Gates {
    /// Frame between the two selected nodes.
    pub frame: Option<&'static str>,
    /// Shell on the four selected nodes.
    pub shell: Option<&'static str>,
    /// The Frame tool: needs a material, a section, and nodes to click.
    pub frame_tool: Option<&'static str>,
    pub shell_tool: Option<&'static str>,
    pub group: Option<&'static str>,
    pub diaphragm: Option<&'static str>,
    pub nodal_load: Option<&'static str>,
    pub distributed_load: Option<&'static str>,
}

impl Gates {
    pub fn of(document: &Document) -> Self {
        let model = document.model();
        let nodes = document.selected_of(EntityKind::Node).len();
        let frames = document.selected_of(EntityKind::Frame).len();
        let no_material = model
            .materials
            .is_empty()
            .then_some("Define a material first");
        let no_section = model.sections.is_empty().then_some("Define a section first");
        let no_case = model
            .load_cases
            .is_empty()
            .then_some("Define a load case first");
        Self {
            frame: no_material
                .or(no_section)
                .or((nodes != 2).then_some("Select exactly two nodes, I then J")),
            shell: no_material
                .or((nodes != 4).then_some("Select exactly four nodes, going around the shell")),
            frame_tool: no_material
                .or(no_section)
                .or((model.nodes.len() < 2).then_some("Place at least two nodes first")),
            shell_tool: no_material
                .or((model.nodes.len() < 4).then_some("Place at least four nodes first")),
            group: document
                .selection()
                .is_empty()
                .then_some("Select the members of the group first"),
            diaphragm: (nodes < 2).then_some("Select two or more nodes first"),
            nodal_load: no_case.or((nodes == 0).then_some("Select the nodes to load first")),
            distributed_load: no_case
                .or((frames == 0).then_some("Select the frames to load first")),
        }
    }
}

/// "2 nodes, 1 frame" for the current selection, or "Nothing".
pub fn selection_summary(document: &Document) -> String {
    let model = document.model();
    let mut counts: Vec<(EntityKind, usize)> = vec![];
    for id in document.selection() {
        if let Some(kind) = model.kind_of(*id) {
            match counts.iter_mut().find(|(k, _)| *k == kind) {
                Some((_, n)) => *n += 1,
                None => counts.push((kind, 1)),
            }
        }
    }
    if counts.is_empty() {
        return "Nothing".into();
    }
    counts
        .iter()
        .map(|(k, n)| format!("{n} {k}{}", if *n == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Combinations solved by the last analysis and which one is shown.
pub struct AnalysisSummary {
    pub combinations: Vec<SharedString>,
    pub shown: usize,
}

/// Everything the ribbon and prompt strip read, gathered by the workspace.
pub struct RibbonState {
    pub can_undo: bool,
    pub can_redo: bool,
    pub has_selection: bool,
    pub selection: String,
    pub gates: Gates,
    pub options: DisplayOptions,
    pub tool: Tool,
    /// Names of the nodes the draw tool has taken so far.
    pub picked: Vec<String>,
    pub results: ResultsState,
    pub analysis: Option<AnalysisSummary>,
    /// No nodes yet.
    pub empty: bool,
    /// The material and section a drawn frame gets, when both exist.
    pub defaults: Option<(String, String)>,
    /// Looking at an elevation, where the ground plane is edge-on.
    pub in_elevation: bool,
}

// MARK: Building blocks

/// A tooltip: the description alone, or with the reason the command is gated.
fn tip(description: &str, gate: Option<&str>) -> String {
    match gate {
        None => description.into(),
        Some(reason) => format!("{description}. {reason}."),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Plain,
    /// The tool in use, or an option that is on.
    Active,
    /// The one primary action: Run.
    Primary,
}

/// A large ribbon button: icon over label, greyed while `gate` gives a reason.
// Every call site reads as a table row, which a builder struct would only blur.
#[allow(clippy::too_many_arguments)]
fn big(
    id: &'static str,
    label: &'static str,
    icon: Lucide,
    description: &str,
    gate: Option<&'static str>,
    look: Look,
    action: Box<dyn Action>,
    theme: &Theme,
) -> AnyElement {
    let enabled = gate.is_none();
    let text = tip(description, gate);
    let tip_action = action.boxed_clone();
    let (fg, bg, border, hover) = match look {
        Look::Plain => (
            theme.foreground,
            theme.transparent,
            theme.transparent,
            theme.list_hover,
        ),
        Look::Active => (
            theme.primary,
            theme.list_active,
            theme.list_active_border,
            theme.list_active,
        ),
        Look::Primary => (
            theme.primary_foreground,
            theme.primary,
            theme.primary,
            theme.primary_hover,
        ),
    };
    div()
        .id(id)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .min_w(px(62.))
        .px_1p5()
        .h(px(56.))
        .rounded_md()
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .when(!enabled, |this| this.opacity(0.4))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        })
        .tooltip(move |window, cx| {
            Tooltip::new(text.clone())
                .action(tip_action.as_ref(), None)
                .build(window, cx)
        })
        .child(Icon::new(icon).size(px(20.)))
        .child(div().text_xs().whitespace_nowrap().child(label))
        .into_any_element()
}

/// A small icon-only button for the File and Edit grids.
fn small(
    id: &'static str,
    icon: Lucide,
    description: &'static str,
    enabled: bool,
    action: Box<dyn Action>,
) -> AnyElement {
    Button::new(id)
        .xsmall()
        .ghost()
        .icon(Icon::new(icon))
        .tooltip_with_action(description, action.as_ref(), None)
        .disabled(!enabled)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        .into_any_element()
}

/// Two rows of two small buttons, the height of one big button.
fn grid(buttons: [AnyElement; 4]) -> AnyElement {
    let [a, b, c, d] = buttons;
    v_flex()
        .gap_0p5()
        .justify_center()
        .h(px(56.))
        .child(h_flex().gap_0p5().child(a).child(b))
        .child(h_flex().gap_0p5().child(c).child(d))
        .into_any_element()
}

/// Buttons in a row with a caption underneath.
fn group(
    caption: &'static str,
    muted: Hsla,
    children: impl IntoIterator<Item = AnyElement>,
) -> AnyElement {
    v_flex()
        .items_center()
        .justify_between()
        .gap_1()
        .px_1()
        .child(h_flex().gap_0p5().items_start().children(children))
        .child(div().text_xs().text_color(muted).child(caption))
        .into_any_element()
}

fn divider(border: Hsla) -> AnyElement {
    div().w(px(1.)).my_1().bg(border).into_any_element()
}

/// The small caret beside Material and Section that offers the custom variant.
fn more(
    id: &'static str,
    tooltip: &'static str,
    library: Box<dyn Action>,
    custom: Box<dyn Action>,
) -> AnyElement {
    let button = Button::new(id)
        .xsmall()
        .ghost()
        .icon(Icon::new(Lucide::ChevronDown))
        .tooltip(tooltip)
        .dropdown_menu(move |menu, _, _| {
            menu.menu("From library…", library.boxed_clone())
                .menu("Custom…", custom.boxed_clone())
        });
    v_flex()
        .h(px(56.))
        .justify_center()
        .child(button)
        .into_any_element()
}

// MARK: Groups

fn file_group(muted: Hsla) -> AnyElement {
    group(
        "File",
        muted,
        [grid([
            small("new", Lucide::FilePlus, "Start an empty model", true, Box::new(NewModel)),
            small("open", Lucide::FolderOpen, "Open a model file", true, Box::new(OpenModel)),
            small("save", Lucide::Save, "Save the model", true, Box::new(SaveModel)),
            small(
                "save-as",
                Lucide::SaveAll,
                "Save the model to a new file",
                true,
                Box::new(SaveModelAs),
            ),
        ])],
    )
}

fn edit_group(state: &RibbonState, muted: Hsla) -> AnyElement {
    group(
        "Edit",
        muted,
        [grid([
            small("undo", Lucide::Undo2, "Undo the last change", state.can_undo, Box::new(Undo)),
            small("redo", Lucide::Redo2, "Redo the undone change", state.can_redo, Box::new(Redo)),
            small(
                "delete",
                Lucide::Trash,
                "Delete the selection, with the frames and shells on deleted nodes",
                state.has_selection,
                Box::new(DeleteSelected),
            ),
            small(
                "select-all",
                Lucide::SquareDashed,
                "Select every node, frame, and shell",
                true,
                Box::new(SelectAll),
            ),
        ])],
    )
}

fn draw_group(state: &RibbonState, theme: &Theme) -> AnyElement {
    let active = |tool: Tool| {
        if state.tool == tool {
            Look::Active
        } else {
            Look::Plain
        }
    };
    group(
        "Draw",
        theme.muted_foreground,
        [
            big(
                "tool-select",
                "Select",
                Lucide::MousePointer2,
                "Click to select; shift+click adds to the selection",
                None,
                active(Tool::Select),
                Box::new(SelectTool),
                theme,
            ),
            big(
                "tool-node",
                "Node",
                Lucide::CircleDot,
                "Click in the view to place a node on the ground plane",
                None,
                active(Tool::Node),
                Box::new(NodeTool),
                theme,
            ),
            big(
                "tool-frame",
                "Frame",
                Lucide::Slash,
                "Click node I then node J to draw a frame. With two nodes selected, draws between them at once",
                state.gates.frame_tool,
                active(Tool::Frame),
                Box::new(FrameTool),
                theme,
            ),
            big(
                "tool-shell",
                "Shell",
                Lucide::Square,
                "Click four nodes around a shell to draw it. With four nodes selected, draws on them at once",
                state.gates.shell_tool,
                active(Tool::Shell),
                Box::new(ShellTool),
                theme,
            ),
        ],
    )
}

fn define_group(theme: &Theme) -> AnyElement {
    group(
        "Define",
        theme.muted_foreground,
        [
            big(
                "material",
                "Material",
                Lucide::Layers,
                "Add a material from the library",
                None,
                Look::Plain,
                Box::new(AddMaterialFromLibrary),
                theme,
            ),
            more(
                "material-more",
                "Material from the library, or a custom one",
                Box::new(AddMaterialFromLibrary),
                Box::new(AddCustomMaterial),
            ),
            big(
                "section",
                "Section",
                Lucide::Cuboid,
                "Add a section from the library",
                None,
                Look::Plain,
                Box::new(AddSectionFromLibrary),
                theme,
            ),
            more(
                "section-more",
                "Section from the library, or a custom one",
                Box::new(AddSectionFromLibrary),
                Box::new(AddCustomSection),
            ),
            big(
                "load-case",
                "Load case",
                Lucide::FolderInput,
                "Add an empty load case",
                None,
                Look::Plain,
                Box::new(AddLoadCase),
                theme,
            ),
            big(
                "combination",
                "Combination",
                Lucide::Sigma,
                "Add a load combination",
                None,
                Look::Plain,
                Box::new(AddCombination),
                theme,
            ),
        ],
    )
}

fn assign_group(state: &RibbonState, theme: &Theme) -> AnyElement {
    let gates = &state.gates;
    group(
        "Assign to selection",
        theme.muted_foreground,
        [
            big(
                "nodal-load",
                "Nodal load",
                Lucide::ArrowDownToDot,
                "Put a point load on every selected node",
                gates.nodal_load,
                Look::Plain,
                Box::new(AddNodalLoad),
                theme,
            ),
            big(
                "uniform-load",
                "Uniform",
                Lucide::ChevronsDown,
                "Put a uniform load on every selected frame",
                gates.distributed_load,
                Look::Plain,
                Box::new(AddDistributedLoad),
                theme,
            ),
            big(
                "add-group",
                "Group",
                Lucide::Group,
                "Make a named group of the selection",
                gates.group,
                Look::Plain,
                Box::new(AddGroupFromSelection),
                theme,
            ),
            big(
                "add-diaphragm",
                "Diaphragm",
                Lucide::Grid3x3,
                "Constrain the selected nodes as a rigid diaphragm",
                gates.diaphragm,
                Look::Plain,
                Box::new(AddDiaphragmFromSelection),
                theme,
            ),
        ],
    )
}

fn analyze_group(state: &RibbonState, theme: &Theme) -> AnyElement {
    let not_solved = (state.results != ResultsState::Current).then_some("Run the analysis first");
    let combination = match &state.analysis {
        Some(analysis) if !analysis.combinations.is_empty() => {
            let combinations = analysis.combinations.clone();
            let shown = analysis.shown.min(combinations.len() - 1);
            Button::new("show-combination")
                .small()
                .ghost()
                .icon(Icon::new(Lucide::ChartLine))
                .label(combinations[shown].clone())
                .dropdown_caret(true)
                .tooltip("Choose the combination whose deformed shape is shown")
                .dropdown_menu(move |menu, _, _| {
                    combinations
                        .iter()
                        .enumerate()
                        .fold(menu, |menu, (ix, name)| {
                            menu.menu_with_check(
                                name.clone(),
                                ix == shown,
                                Box::new(ShowCombination(name.clone())),
                            )
                        })
                })
                .into_any_element()
        }
        _ => Button::new("show-combination")
            .small()
            .ghost()
            .icon(Icon::new(Lucide::ChartLine))
            .label("Combination")
            .dropdown_caret(true)
            .tooltip(tip(
                "Choose the combination whose deformed shape is shown",
                not_solved,
            ))
            .disabled(true)
            .into_any_element(),
    };
    group(
        "Analyze",
        theme.muted_foreground,
        [
            big(
                "run",
                "Run",
                Lucide::Play,
                "Solve every load combination",
                None,
                Look::Primary,
                Box::new(RunStaticAnalysis),
                theme,
            ),
            big(
                "deformed",
                "Deformed",
                Lucide::Activity,
                "Overlay the deformed shape, scaled to 5% of the model size",
                not_solved,
                if state.options.deformed && not_solved.is_none() {
                    Look::Active
                } else {
                    Look::Plain
                },
                Box::new(ToggleDeformedShape),
                theme,
            ),
            v_flex()
                .h(px(56.))
                .justify_center()
                .child(combination)
                .into_any_element(),
        ],
    )
}

/// Whether the results still describe the model, as a coloured chip.
fn results_chip(state: &RibbonState, theme: &Theme) -> AnyElement {
    let (label, color, tooltip) = match state.results {
        ResultsState::Current => (
            "Results current",
            theme.success,
            "The deformed shape describes the model as it is now",
        ),
        ResultsState::Stale => (
            "Results out of date",
            theme.warning,
            "The model changed since the last run. Run again to refresh the results",
        ),
        ResultsState::None => (
            "No results yet",
            theme.muted_foreground,
            "Run solves every load combination",
        ),
    };
    let action: Box<dyn Action> = Box::new(RunStaticAnalysis);
    h_flex()
        .id("results")
        .gap_2()
        .h(px(28.))
        .px_3()
        .rounded_full()
        .border_1()
        .border_color(color.opacity(0.4))
        .bg(color.opacity(0.1))
        .text_xs()
        .text_color(color)
        .whitespace_nowrap()
        .tooltip(move |window, cx| {
            Tooltip::new(tooltip)
                .action(action.as_ref(), None)
                .build(window, cx)
        })
        .child(div().size(px(8.)).rounded_full().bg(color))
        .child(label)
        .into_any_element()
}

pub fn render_ribbon(state: RibbonState, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (muted, border) = (theme.muted_foreground, theme.border);
    let groups = [
        file_group(muted),
        edit_group(&state, muted),
        draw_group(&state, theme),
        define_group(theme),
        assign_group(&state, theme),
        analyze_group(&state, theme),
    ];
    let mut row: Vec<AnyElement> = vec![];
    for (ix, group) in groups.into_iter().enumerate() {
        if ix > 0 {
            row.push(divider(border));
        }
        row.push(group);
    }
    h_flex()
        .w_full()
        .px_2()
        .py_1()
        .gap_1()
        .items_stretch()
        .border_b_1()
        .border_color(border)
        .children(row)
        .child(
            h_flex()
                .ml_auto()
                .pl_2()
                .pb_4()
                .gap_2()
                .items_center()
                .child(results_chip(&state, theme))
                .child(
                    Button::new("search")
                        .small()
                        .outline()
                        .icon(Icon::new(Lucide::Search))
                        .tooltip_with_action("Search every command", &OpenCommandPalette, None)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(OpenCommandPalette), cx)
                        }),
                ),
        )
}

// MARK: Prompt strip

struct Prompt {
    title: String,
    text: String,
    aside: Option<String>,
    /// Which pick the draw tool is waiting for.
    step: Option<usize>,
    /// A tool other than Select is in use.
    active: bool,
}

fn prompt(state: &RibbonState) -> Prompt {
    match state.tool {
        Tool::Select if state.empty => Prompt {
            title: "Select".into(),
            text: "The model is empty. Pick a start in the view, or choose the Node tool and click to place nodes.".into(),
            aside: None,
            step: None,
            active: false,
        },
        Tool::Select if !state.has_selection => Prompt {
            title: "Select".into(),
            text: "Click a node, frame, or shell. Shift+click adds to the selection.".into(),
            aside: state.analysis.as_ref().and_then(|analysis| {
                (state.options.deformed && !analysis.combinations.is_empty()).then(|| {
                    let shown = analysis.shown.min(analysis.combinations.len() - 1);
                    format!("Showing deformed shape for {}", analysis.combinations[shown])
                })
            }),
            step: None,
            active: false,
        },
        Tool::Select => {
            let gates = &state.gates;
            let can: Vec<&str> = [
                (gates.frame, "draw a frame between them, I then J"),
                (gates.shell, "draw a shell on them"),
                (gates.nodal_load, "assign a nodal load"),
                (gates.distributed_load, "assign a uniform load"),
                (gates.group, "make a group"),
                (gates.diaphragm, "constrain a diaphragm"),
            ]
            .into_iter()
            .filter_map(|(gate, verb)| gate.is_none().then_some(verb))
            .collect();
            Prompt {
                title: format!("{} selected", state.selection),
                text: if can.is_empty() {
                    "Edit them in the panel on the right, or press Delete.".into()
                } else {
                    format!("You can {}. Delete removes them.", can.join(", "))
                },
                aside: Some("Esc clears the selection".into()),
                step: None,
                active: false,
            }
        }
        Tool::Node => Prompt {
            title: "Node".into(),
            text: if state.in_elevation {
                "Click to place a node in the view plane through the centre, snapped to 0.25 m. Esc returns to Select.".into()
            } else {
                "Click empty space to place a node on the ground plane, snapped to 0.25 m. Click a node to select it. Esc returns to Select.".into()
            },
            aside: None,
            step: None,
            active: true,
        },
        Tool::Frame => Prompt {
            title: "Frame".into(),
            text: match state.picked.last() {
                None => "Click the I node, then the J node.".into(),
                Some(name) => format!(
                    "Click the J node. I = {name}. The next frame starts from J; Esc stops."
                ),
            },
            aside: state
                .defaults
                .as_ref()
                .map(|(material, section)| format!("Section {section} · Material {material}")),
            step: Some(state.picked.len().min(1) + 1),
            active: true,
        },
        Tool::Shell => Prompt {
            title: "Shell".into(),
            text: format!(
                "Click the four corner nodes in order around the shell. {} of 4 picked. Esc starts over.",
                state.picked.len()
            ),
            aside: state
                .defaults
                .as_ref()
                .map(|(material, _)| format!("Material {material}")),
            step: Some((state.picked.len() + 1).min(4)),
            active: true,
        },
    }
}

/// The strip above the viewport that says what the current tool wants next.
pub fn render_prompt(state: &RibbonState, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let p = prompt(state);
    let (bg, fg, border, body) = if p.active {
        (
            theme.list_active,
            theme.primary,
            theme.list_active_border,
            theme.primary,
        )
    } else {
        (
            theme.sidebar,
            theme.foreground,
            theme.border,
            theme.muted_foreground,
        )
    };
    let (primary, on_primary, muted) = (theme.primary, theme.primary_foreground, theme.muted_foreground);
    h_flex()
        .w_full()
        .h(px(32.))
        .px_3()
        .gap_3()
        .items_center()
        .justify_between()
        .bg(bg)
        .border_b_1()
        .border_color(border)
        .text_sm()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .min_w_0()
                .when_some(p.step, |this, step| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(18.))
                            .rounded_full()
                            .bg(primary)
                            .text_color(on_primary)
                            .text_xs()
                            .child(step.to_string()),
                    )
                })
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg)
                        .whitespace_nowrap()
                        .child(p.title),
                )
                .child(div().text_color(body).truncate().child(p.text)),
        )
        .when_some(p.aside, |this, aside| {
            this.child(
                div()
                    .text_xs()
                    .text_color(if p.active { fg } else { muted })
                    .whitespace_nowrap()
                    .child(aside),
            )
        })
}
