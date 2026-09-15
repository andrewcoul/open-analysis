# Desktop GUI Plan

Plan and status for `oa-gui`, the desktop viewer and editor. Written
2026-09-14 alongside the first working build. The target is an ETABS-style
application; this phase covers the basics of viewing and editing a model.

## Stack

- [GPUI Kit](https://gpui-kit.com) 0.6.1 (`gpui-kit` crate), which pins GPUI
  as the published `gpui-pre` snapshot. Everything comes from crates.io; no
  git dependencies.
- Builds and runs on the repository's `windows-gnu` toolchain through
  `scripts/cargo.ps1`. The linker prints a harmless "multiple non-default
  manifests" warning from the GPUI and Kit resource files.
- The application depends on `oa-model` and `oa-core` only. The GUI issues
  the same commands an agent does through `oa-mcp`; no edit bypasses the
  command interface.

## Structure

```
crates/oa-gui/src/
  main.rs        bootstrap, key bindings, routing of every action to the workspace
  actions.rs     the command set: one GPUI action per menu item / shortcut
  document.rs    Editor + path + dirty flag + selection + problems + last analysis
  camera.rs      orthographic orbit camera, pure arithmetic with tests
  viewport.rs    3D canvas: nodes, frames, shells, labels, deformed shape, picking,
                 the draw tools, the view controls overlay, and the start card
  explorer.rs    model tree by entity kind, click to select, double-click to edit;
                 opens as a dialog from View > Model browser
  properties.rs  property editor: fields per kind, commit through Update commands;
                 opens as a dialog from Edit > Properties or a double-click
  dialogs.rs     add node by coordinates, materials and sections (library or custom), loads
  prompt.rs      the prompt strip above the view and the selection gates
  workspace.rs   window layout, menus, command palette, pop-up panels, status bar,
                 file, edit, and tool commands
  text.rs        number formatting and parsing at the UI boundary
```

There is no ribbon and no docked side panel: the window is the menu bar, a
prompt strip, the 3D view, and a status bar. Every command lives in the menu
bar, which reads in the order a model is built (File, Edit, View, Define,
Draw, Assign, Analyze, Help), and in the command palette (Ctrl+K), which
lists every action with its gate reason. Commands that need something first
are disabled through the shared `Gates`. The model tree (Ctrl+B) and the
property editor (Ctrl+E, or a double-click on an entity in the view or the
tree) open as dialogs over the view. The status bar says whether the results
are current, out of date after an edit, or absent. A prompt strip above the
view says what the current tool wants next. View presets, fit, labels, and
the up axis sit in the top-right corner of the view; an empty model shows a
start card in the view. The few Lucide icons the view uses are embedded on
top of the kit's default set in `main.rs`, which also gives both kit themes
an indigo accent.

Drawing is by tool. Node places a node where you click, on the ground plane
snapped to 0.25 m (or in the view plane through the centre when the ground
is edge-on in an elevation). Frame takes node I then node J and carries on
from J; Shell takes four nodes in order. With nodes already selected, the
Frame and Shell buttons draw on them at once. Esc drops the shape being
drawn, then the tool, then the selection. The viewport reports finished
shapes as `ViewportEvent`s and the workspace turns them into commands, so
no edit bypasses the command interface. The Paper file "open-analysis GUI"
(2026-09-15) holds the design this grew from; it still shows the earlier
ribbon and docked panels, which were dropped in favour of menus and dialogs.

`Document` is one GPUI entity that every panel observes. Panels never hold
model state of their own; they read the document in `render` and mutate it
only through `Document::apply`, which runs a command, recompiles for
validation problems, discards results, and notifies observers.

Every user command is a GPUI action registered at application level, so the
menu bar, ribbon, key bindings, and panel buttons dispatch the same thing.
Actions are routed to `Workspace` methods in `main.rs`.

## What works

- Open, save, save as, new, and new example frame, with a discard
  confirmation when the model has unsaved changes. Files are the `oa-model`
  JSON format.
- 3D orthographic view with orbit, pan, zoom about the cursor, four view
  presets, zoom extents, Y-up or Z-up display, node and frame labels, an
  axis triad, and restraint markers. Frames and shells are painted as paths
  on a GPUI canvas; picking uses the last frame's screen positions.
- Selection shared between the view, the model tree, and the property
  panel. Click, shift+click, select all, deselect.
- Property editing for every entity kind: nodes (position, restraints,
  springs, mass), frames (end nodes, material, section, releases, roll,
  axial behaviour), shells, materials, sections, load cases (self weight
  plus a loads list with removal), combinations (terms), diaphragms
  (normal, master, nodes from selection), groups (members from selection).
  Text fields commit on Enter or blur; choices and checkboxes commit at
  once. Every commit is an undoable command.
- Multi-selection: assign a section or material to all selected frames, set
  restraints on all selected nodes, delete.
- Delete cascades: frames and shells on deleted nodes go too, and loads and
  diaphragm entries that reference deleted entities are trimmed first, all
  in one batch that rolls back on failure.
- Define materials and sections from the starter library or by value; add
  load cases, combinations, groups, diaphragms; add nodal loads to selected
  nodes and uniform loads to selected frames.
- Static analysis of every combination through `oa-core`, with the deformed
  shape drawn for a chosen combination and auto-scaled to 5% of the model
  extent. Results are dropped on any edit so a stale shape cannot be shown.
- Validation problems from `compile` in the status bar.

## Not yet done

- Snapping the Node tool to anything but the 0.25 m grid: no snapping to
  existing nodes, frame ends, or a story level, and no box selection.
- Grids and stories. There is no story or grid system, which ETABS relies on
  for plan views and level selection.
- Display units. The model is SI and every field is labelled in SI. The
  `display_units` preference in metadata is not applied.
- Result display beyond the deformed shape: force diagrams, reactions,
  tables, and envelopes. The result store and envelopes exist in
  `oa-results` and are exposed to agents but not to the GUI.
- Tabular editing of whole tables, load display in the view, and box
  selection.
- Large models: the model tree renders plain rows and cuts each section at
  2000 entries; the view repaints every frame on every change. A virtual
  list and cached geometry are the obvious next steps.
- Only one window and one model at a time.

## Verification

- `cargo test -p oa-gui` covers the camera projection, pan, zoom, and fit,
  number formatting and parsing, unused-name generation, and that the
  example frame compiles and solves.
- Rendering was checked by launching the built binary and capturing its
  window (PrintWindow). The action path that menus, toolbar buttons, and
  shortcuts use was checked by dispatching label toggles and the analysis
  through `Window::dispatch_action` at startup and capturing the result. Click
  selection of a node and of a frame, and the property panel they open, were
  checked with one test click each. Property editing, undo, dialogs, menus,
  orbit, file open/save, the draw tools, and the command palette have not
  been exercised interactively yet and are not covered by automated UI tests;
  the 2026-09-15 ribbon rework was verified by build, clippy, the camera
  unprojection tests, and a PrintWindow capture of the launched window. GPUI Kit's `test-support` feature offers
  headless UI tests and is the intended route.
