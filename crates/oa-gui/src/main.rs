//! Desktop model viewer and editor for open-analysis.
//!
//! Layout: one window, one [`Workspace`]. Every command is an action; the
//! menu bar, command palette, and key bindings dispatch actions, and this file routes
//! each one to a workspace method.
mod actions;
mod camera;
mod dialogs;
mod document;
mod explorer;
mod loads;
mod properties;
mod prompt;
mod results;
mod text;
mod viewport;
mod workspace;

use actions::*;
use camera::ViewPreset;
use gpui_kit::component::{Root, TitleBar, WindowExt as _};
use gpui_kit::*;
use workspace::Workspace;

/// Switzer, embedded so the interface looks the same without the font installed.
const FONTS: [&[u8]; 3] = [
    include_bytes!("../assets/fonts/Switzer-Regular.otf"),
    include_bytes!("../assets/fonts/Switzer-Medium.otf"),
    include_bytes!("../assets/fonts/Switzer-Semibold.otf"),
];

/// The visual system on top of the kit's themes: Switzer at 14px, an 8px
/// radius everywhere, and indigo as the accent of both the light and dark
/// themes. The configs are changed rather than the live values so a theme
/// switch keeps them.
fn apply_visual_system(cx: &mut App) {
    use gpui_kit::component::{Theme, ThemeConfig};
    let mode = Theme::global(cx).mode;
    let configs = {
        let theme = Theme::global(cx);
        [theme.light_theme.clone(), theme.dark_theme.clone()]
    };
    for config in configs {
        let dark = config.mode.is_dark();
        let mut config: ThemeConfig = (*config).clone();
        let some = |name: &str| Some(SharedString::from(name.to_string()));
        config.font_family = some("Switzer");
        config.font_size = Some(14.0);
        config.mono_font_family = some("Switzer");
        config.mono_font_size = Some(12.0);
        config.radius = Some(8);
        config.radius_lg = Some(8);
        let (primary, hover, active, soft, selection) = if dark {
            ("indigo-500", "indigo-400", "indigo-600", "indigo-900", "indigo-800")
        } else {
            ("indigo-600", "indigo-700", "indigo-800", "indigo-50", "indigo-100")
        };
        let colors = &mut config.colors;
        colors.primary = some(primary);
        colors.primary_hover = some(hover);
        colors.primary_active = some(active);
        colors.primary_foreground = some("#ffffff");
        colors.button_primary = some(primary);
        colors.button_primary_hover = some(hover);
        colors.button_primary_active = some(active);
        colors.button_primary_foreground = some("#ffffff");
        colors.ring = some(primary);
        colors.link = some(primary);
        colors.caret = some(primary);
        colors.list_active = some(soft);
        colors.list_active_border = some(primary);
        colors.selection = some(selection);
        colors.sidebar_primary = some(primary);
        Theme::global_mut(cx).apply_config(&std::rc::Rc::new(config));
    }
    Theme::change(mode, None, cx);
}

/// The key map, in menu order. Single letters and digits are bound in the
/// `Viewport` key context, so they act only while the view has focus and
/// never fight a text field; everything else holds Ctrl and works anywhere.
fn key_bindings() -> Vec<KeyBinding> {
    const VIEW: Option<&str> = Some("Viewport");
    vec![
        // File
        KeyBinding::new("ctrl-n", NewModel, None),
        KeyBinding::new("ctrl-shift-n", NewExampleModel, None),
        KeyBinding::new("ctrl-o", OpenModel, None),
        KeyBinding::new("ctrl-s", SaveModel, None),
        KeyBinding::new("ctrl-shift-s", SaveModelAs, None),
        KeyBinding::new("ctrl-q", Quit, None),
        // Edit
        KeyBinding::new("ctrl-z", Undo, None),
        KeyBinding::new("ctrl-y", Redo, None),
        KeyBinding::new("ctrl-shift-z", Redo, None),
        KeyBinding::new("ctrl-a", SelectAll, None),
        KeyBinding::new("escape", Cancel, None),
        KeyBinding::new("delete", DeleteSelected, None),
        KeyBinding::new("ctrl-e", ShowProperties, None),
        // View
        KeyBinding::new("ctrl-b", ShowModelBrowser, None),
        KeyBinding::new("ctrl-k", OpenCommandPalette, None),
        KeyBinding::new("1", ViewThreeD, VIEW),
        KeyBinding::new("2", ViewPlan, VIEW),
        KeyBinding::new("3", ViewElevationX, VIEW),
        KeyBinding::new("4", ViewElevationY, VIEW),
        KeyBinding::new("z", ZoomExtents, VIEW),
        KeyBinding::new("shift-n", ToggleNodeLabels, VIEW),
        KeyBinding::new("shift-f", ToggleFrameLabels, VIEW),
        KeyBinding::new("shift-z", ToggleUpAxis, VIEW),
        KeyBinding::new("shift-d", ToggleDeformedShape, VIEW),
        // Define
        KeyBinding::new("ctrl-m", AddMaterialFromLibrary, None),
        KeyBinding::new("ctrl-shift-m", AddCustomMaterial, None),
        KeyBinding::new("ctrl-t", AddSectionFromLibrary, None),
        KeyBinding::new("ctrl-shift-t", AddCustomSection, None),
        KeyBinding::new("ctrl-l", ShowLoadCases, None),
        KeyBinding::new("ctrl-shift-l", ShowCombinations, None),
        // Draw
        KeyBinding::new("v", SelectTool, VIEW),
        KeyBinding::new("n", NodeTool, VIEW),
        KeyBinding::new("f", FrameTool, VIEW),
        KeyBinding::new("s", ShellTool, VIEW),
        KeyBinding::new("shift-a", AddNode, VIEW),
        // Assign
        KeyBinding::new("l", AddNodalLoad, VIEW),
        KeyBinding::new("u", AddDistributedLoad, VIEW),
        KeyBinding::new("g", AddGroupFromSelection, VIEW),
        KeyBinding::new("d", AddDiaphragmFromSelection, VIEW),
        // Analyze
        KeyBinding::new("ctrl-r", RunStaticAnalysis, None),
        KeyBinding::new("ctrl-]", NextCombination, None),
        KeyBinding::new("ctrl-[", PreviousCombination, None),
        // Help
        KeyBinding::new("f1", About, None),
    ]
}

/// Registers an app-level handler that runs `f` on the workspace in its window.
///
/// Actions arrive while the window is checked out for its own update, so the
/// window cannot be updated again until that returns. The work is deferred to
/// the end of the current effect cycle, when the window is available.
fn route<A: Action + Clone>(
    cx: &mut App,
    window: WindowHandle<Root>,
    workspace: Entity<Workspace>,
    f: impl Fn(&mut Workspace, &A, &mut Window, &mut Context<Workspace>) + 'static,
) {
    // The window is updated through its untyped handle: the typed one leases
    // the `Root` view, which the notification and dialog layers lease again.
    let window: AnyWindowHandle = window.into();
    let f = std::rc::Rc::new(f);
    cx.on_action(move |action: &A, cx: &mut App| {
        let (action, workspace, f) = (action.clone(), workspace.clone(), f.clone());
        cx.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |workspace, cx| f(workspace, &action, window, cx))
                })
                .expect("the main window outlives its action handlers");
        });
    });
}

fn route_all(cx: &mut App, window: WindowHandle<Root>, workspace: Entity<Workspace>) {
    macro_rules! on {
        ($action:ty, $body:expr) => {
            route::<$action>(cx, window, workspace.clone(), $body)
        };
    }
    on!(NewModel, |ws, _, window, cx| ws.new_model(window, cx));
    on!(NewExampleModel, |ws, _, window, cx| ws
        .new_example(window, cx));
    on!(OpenModel, |ws, _, window, cx| ws.open(window, cx));
    on!(SaveModel, |ws, _, window, cx| ws.save(window, cx));
    on!(SaveModelAs, |ws, _, window, cx| ws.save_as(window, cx));
    on!(Undo, |ws, _, window, cx| ws.undo(window, cx));
    on!(Redo, |ws, _, window, cx| ws.redo(window, cx));
    on!(SelectAll, |ws, _, _, cx| ws.select_all(cx));
    on!(DeselectAll, |ws, _, _, cx| ws.deselect_all(cx));
    on!(DeleteSelected, |ws, _, window, cx| ws
        .delete_selected(window, cx));
    on!(AddNode, |ws, _, window, cx| dialogs::add_node(
        ws.document().clone(),
        window,
        cx
    ));
    on!(AddFrameBetweenSelected, |ws, _, window, cx| ws
        .add_frame_between_selected(window, cx));
    on!(AddShellFromSelected, |ws, _, window, cx| ws
        .add_shell_from_selected(window, cx));
    on!(AddMaterialFromLibrary, |ws, _, window, cx| {
        dialogs::add_material_from_library(ws.document().clone(), window, cx)
    });
    on!(AddCustomMaterial, |ws, _, window, cx| {
        dialogs::add_custom_material(ws.document().clone(), window, cx)
    });
    on!(AddSectionFromLibrary, |ws, _, window, cx| {
        dialogs::add_section_from_library(ws.document().clone(), window, cx)
    });
    on!(AddCustomSection, |ws, _, window, cx| {
        dialogs::add_custom_section(ws.document().clone(), window, cx)
    });
    on!(ShowLoadCases, |ws, _, window, cx| ws
        .show_load_cases(window, cx));
    on!(ShowCombinations, |ws, _, window, cx| ws
        .show_combinations(window, cx));
    on!(AddLoadCase, |ws, _, window, cx| ws
        .add_load_case(window, cx));
    on!(AddAsceLoadCase, |ws, _, window, cx| {
        dialogs::add_asce_load_case(ws.document().clone(), window, cx)
    });
    on!(AddCombination, |ws, _, window, cx| ws
        .add_combination(window, cx));
    on!(GenerateCombinations, |ws, _, window, cx| {
        dialogs::generate_combinations(ws.document().clone(), window, cx)
    });
    on!(AddGroupFromSelection, |ws, _, window, cx| ws
        .add_group_from_selection(window, cx));
    on!(AddDiaphragmFromSelection, |ws, _, window, cx| ws
        .add_diaphragm_from_selection(window, cx));
    on!(AddNodalLoad, |ws, _, window, cx| dialogs::add_nodal_load(
        ws.document().clone(),
        window,
        cx
    ));
    on!(AddDistributedLoad, |ws, _, window, cx| {
        dialogs::add_distributed_load(ws.document().clone(), window, cx)
    });
    on!(ViewThreeD, |ws, _, _, cx| ws
        .set_view(ViewPreset::ThreeD, cx));
    on!(ViewPlan, |ws, _, _, cx| ws.set_view(ViewPreset::Plan, cx));
    on!(ViewElevationX, |ws, _, _, cx| ws
        .set_view(ViewPreset::ElevationX, cx));
    on!(ViewElevationY, |ws, _, _, cx| ws
        .set_view(ViewPreset::ElevationY, cx));
    on!(ZoomExtents, |ws, _, _, cx| ws.zoom_extents(cx));
    on!(ToggleNodeLabels, |ws, _, _, cx| ws
        .toggle_option(|o| o.node_labels = !o.node_labels, cx));
    on!(ToggleFrameLabels, |ws, _, _, cx| ws
        .toggle_option(|o| o.frame_labels = !o.frame_labels, cx));
    on!(ToggleDeformedShape, |ws, _, _, cx| ws
        .toggle_option(|o| o.deformed = !o.deformed, cx));
    on!(ToggleUpAxis, |ws, _, _, cx| ws.toggle_up_axis(cx));
    on!(RunStaticAnalysis, |ws, _, window, cx| ws
        .run_static(window, cx));
    on!(NextCombination, |ws, _, _, cx| ws.step_combination(1, cx));
    on!(PreviousCombination, |ws, _, _, cx| ws
        .step_combination(-1, cx));
    on!(ShowCombination, |ws, action: &ShowCombination, _, cx| ws
        .show_combination(&action.0, cx));
    on!(SelectTool, |ws, _, _, cx| ws.set_tool(viewport::Tool::Select, cx));
    on!(NodeTool, |ws, _, _, cx| ws.set_tool(viewport::Tool::Node, cx));
    on!(FrameTool, |ws, _, window, cx| ws.use_frame_tool(window, cx));
    on!(ShellTool, |ws, _, window, cx| ws.use_shell_tool(window, cx));
    on!(Cancel, |ws, _, window, cx| ws.cancel(window, cx));
    on!(OpenCommandPalette, |ws, _, window, cx| ws
        .open_palette(window, cx));
    on!(ShowProperties, |ws, _, window, cx| ws
        .show_properties(window, cx));
    on!(ShowModelBrowser, |ws, _, window, cx| ws
        .show_model_browser(window, cx));
    on!(ShowMemberResults, |ws, _, window, cx| ws
        .show_member_results(window, cx));
    on!(ShowDiagram, |ws, action: &ShowDiagram, _, cx| ws
        .show_diagram(action.0, cx));
    on!(About, |ws, _, window, cx| ws.about(window, cx));
    cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            cx.text_system()
                .add_fonts(FONTS.iter().map(|f| std::borrow::Cow::Borrowed(*f)).collect())
                .expect("the embedded fonts load");
            gpui_kit::init(cx);
            apply_visual_system(cx);
            cx.bind_keys(key_bindings());
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1440.), px(900.)),
                    cx,
                ))),
                ..TitleBar::window_options()
            };
            let window = cx
                .open_window(options, |window, cx| {
                    let workspace = cx.new(|cx| Workspace::new(window, cx));
                    let document = workspace.read(cx).document().clone();
                    window.on_window_should_close(cx, move |window, cx| {
                        if !document.read(cx).is_dirty() {
                            return true;
                        }
                        window.open_alert_dialog(cx, |alert, _, _| {
                            alert
                                .title("Quit without saving?")
                                .description("The model has changes that have not been saved.")
                                .confirm()
                                .on_ok(|_, window, _| {
                                    window.remove_window();
                                    true
                                })
                        });
                        false
                    });
                    cx.new(|cx| Root::new(workspace, window, cx))
                })
                .expect("open the main window");
            let workspace = window
                .read(cx)
                .expect("window was just opened")
                .view()
                .clone()
                .downcast::<Workspace>()
                .expect("root holds the workspace");
            route_all(cx, window, workspace);
        });
}
