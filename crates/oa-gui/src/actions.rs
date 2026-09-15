//! Every command the GUI offers, so menus, toolbar buttons, key bindings, and
//! panels all dispatch the same actions to the workspace.
use gpui_kit::*;
use serde::Deserialize;

actions!(
    oa_gui,
    [
        NewModel,
        NewExampleModel,
        OpenModel,
        SaveModel,
        SaveModelAs,
        Quit,
        Undo,
        Redo,
        SelectAll,
        DeselectAll,
        DeleteSelected,
        AddNode,
        AddFrameBetweenSelected,
        AddShellFromSelected,
        AddMaterialFromLibrary,
        AddCustomMaterial,
        AddSectionFromLibrary,
        AddCustomSection,
        AddLoadCase,
        AddCombination,
        AddGroupFromSelection,
        AddDiaphragmFromSelection,
        AddNodalLoad,
        AddDistributedLoad,
        ViewThreeD,
        ViewPlan,
        ViewElevationX,
        ViewElevationY,
        ZoomExtents,
        ToggleNodeLabels,
        ToggleFrameLabels,
        ToggleUpAxis,
        ToggleDeformedShape,
        RunStaticAnalysis,
        SelectTool,
        NodeTool,
        FrameTool,
        ShellTool,
        Cancel,
        OpenCommandPalette,
        About,
    ]
);

/// Shows the deformed shape for the combination with this name.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = oa_gui, no_json)]
pub struct ShowCombination(pub SharedString);
