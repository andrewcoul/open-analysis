//! The prompt strip above the viewport, which says what the current tool
//! wants next, and the [`Gates`] that decide which commands can run. Every
//! command lives in the menu bar and the command palette; a command that
//! needs something first is disabled there, and the palette shows the reason.
use crate::document::Document;
use crate::viewport::{DisplayOptions, Tool};
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use oa_model::EntityKind;

/// Why each gated command is unavailable, or `None` when it can run.
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
    /// Generating ASCE 7 combinations needs cases to build them from.
    pub generate: Option<&'static str>,
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
            generate: no_case,
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

/// Everything the prompt strip reads, gathered by the workspace.
pub struct PromptState {
    pub has_selection: bool,
    pub selection: String,
    pub gates: Gates,
    pub options: DisplayOptions,
    pub tool: Tool,
    /// Names of the nodes the draw tool has taken so far.
    pub picked: Vec<String>,
    pub analysis: Option<AnalysisSummary>,
    /// No nodes yet.
    pub empty: bool,
    /// The material and section a drawn frame gets, when both exist.
    pub defaults: Option<(String, String)>,
    /// Looking at an elevation, where the ground plane is edge-on.
    pub in_elevation: bool,
}

struct Prompt {
    title: String,
    text: String,
    aside: Option<String>,
    /// A tool other than Select is in use.
    active: bool,
}

fn prompt(state: &PromptState) -> Prompt {
    match state.tool {
        Tool::Select if state.empty => Prompt {
            title: "Select".into(),
            text: "The model is empty. Pick a start in the view, or choose Draw > Node tool and click to place nodes.".into(),
            aside: None,
            active: false,
        },
        Tool::Select if !state.has_selection => Prompt {
            title: "Select".into(),
            text: "Click a node, frame, or shell. Shift+click adds to the selection; double-click opens its properties.".into(),
            aside: state.analysis.as_ref().and_then(|analysis| {
                (state.options.deformed && !analysis.combinations.is_empty()).then(|| {
                    let shown = analysis.shown.min(analysis.combinations.len() - 1);
                    format!("Showing deformed shape for {}", analysis.combinations[shown])
                })
            }),
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
                    "Double-click or press Ctrl+E to edit, or Delete to remove.".into()
                } else {
                    format!(
                        "You can {}. Double-click or Ctrl+E edits, Delete removes.",
                        can.join(", ")
                    )
                },
                aside: Some("Esc clears the selection".into()),
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
            active: true,
        },
    }
}

/// The strip above the viewport that says what the current tool wants next.
pub fn render_prompt(state: &PromptState, cx: &App) -> impl IntoElement {
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
            theme.background,
            theme.foreground,
            theme.border,
            theme.muted_foreground,
        )
    };
    let muted = theme.muted_foreground;
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
                .child(div().text_color(fg).whitespace_nowrap().child(p.title))
                .child(div().text_color(body).truncate().child(p.text)),
        )
        .when_some(p.aside, |this, aside| {
            this.child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if p.active { fg } else { muted })
                    .whitespace_nowrap()
                    .child(aside),
            )
        })
}
