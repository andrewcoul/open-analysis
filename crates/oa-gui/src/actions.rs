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
        ImportCad,
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
        ViewWholeModel,
        ViewActiveLevel,
        ViewActiveLevelContext,
        LevelUp,
        LevelDown,
        ShowLevels,
        ToggleNodeLabels,
        ToggleFrameLabels,
        ToggleUnderlays,
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
        ShowMemberResults,
        About,
    ]
);

/// Shows the deformed shape for the combination with this name.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = oa_gui, no_json)]
pub struct ShowCombination(pub SharedString);

/// Draws this section force along every member, or none of them.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = oa_gui, no_json)]
pub struct ShowDiagram(pub Option<crate::results::Diagram>);

/// Switches one object snap of the draw tools on or off.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = oa_gui, no_json)]
pub struct ToggleSnap(pub crate::snap::Kind);

/// Shows every number with this many decimals.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = oa_gui, no_json)]
pub struct SetPrecision(pub usize);

/// Makes the level with this entity id the active one: where the Node tool
/// places nodes and what the level views show.
#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = oa_gui, no_json)]
pub struct SetActiveLevel(pub u64);
