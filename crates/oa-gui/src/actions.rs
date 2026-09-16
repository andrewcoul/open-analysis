//! Every command the GUI offers, so menus, the command palette, key bindings,
//! and view overlays all dispatch the same actions to the workspace.
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
        ShowLoadCases,
        ShowCombinations,
        AddLoadCase,
        AddAsceLoadCase,
        AddCombination,
        GenerateCombinations,
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
        NextCombination,
        PreviousCombination,
        SelectTool,
        NodeTool,
        FrameTool,
        ShellTool,
        Cancel,
        OpenCommandPalette,
        ShowProperties,
        ShowModelBrowser,
        About,
    ]
);

/// Shows the deformed shape for the combination with this name.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = oa_gui, no_json)]
pub struct ShowCombination(pub SharedString);
