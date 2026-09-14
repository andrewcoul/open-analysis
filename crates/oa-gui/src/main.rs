//! Desktop model viewer and editor for open-analysis.
//!
//! Layout: one window, one [`Workspace`]. Every command is an action; the
//! menu bar, toolbar, and key bindings dispatch actions, and this file routes
//! each one to a workspace method.
mod actions;
mod camera;
mod dialogs;
mod document;
mod explorer;
mod properties;
mod text;
mod viewport;
mod workspace;

use actions::*;
use camera::ViewPreset;
use gpui_kit::component::{Root, TitleBar, WindowExt as _};
use gpui_kit::*;
use workspace::Workspace;

fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("ctrl-n", NewModel, None),
        KeyBinding::new("ctrl-o", OpenModel, None),
        KeyBinding::new("ctrl-s", SaveModel, None),
        KeyBinding::new("ctrl-shift-s", SaveModelAs, None),
        KeyBinding::new("ctrl-z", Undo, None),
        KeyBinding::new("ctrl-y", Redo, None),
        KeyBinding::new("ctrl-shift-z", Redo, None),
        KeyBinding::new("ctrl-a", SelectAll, None),
        KeyBinding::new("escape", DeselectAll, None),
        KeyBinding::new("delete", DeleteSelected, None),
        KeyBinding::new("f5", RunStaticAnalysis, None),
        KeyBinding::new("f2", ZoomExtents, None),
        KeyBinding::new("ctrl-1", ViewThreeD, None),
        KeyBinding::new("ctrl-2", ViewPlan, None),
        KeyBinding::new("ctrl-3", ViewElevationX, None),
        KeyBinding::new("ctrl-4", ViewElevationY, None),
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
    on!(AddLoadCase, |ws, _, window, cx| ws
        .add_load_case(window, cx));
    on!(AddCombination, |ws, _, window, cx| ws
        .add_combination(window, cx));
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
    on!(ShowCombination, |ws, action: &ShowCombination, _, cx| ws
        .show_combination(&action.0, cx));
    on!(About, |ws, _, window, cx| ws.about(window, cx));
    cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
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
