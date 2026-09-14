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
  viewport.rs    3D canvas: nodes, frames, shells, labels, deformed shape, picking
  explorer.rs    model tree by entity kind, click to select
  properties.rs  property panel: fields per kind, commit through Update commands
  dialogs.rs     add node, materials and sections (library or custom), loads
  workspace.rs   window layout, menus, toolbar, status bar, file and edit commands
  text.rs        number formatting and parsing at the UI boundary
```

`Document` is one GPUI entity that every panel observes. Panels never hold
model state of their own; they read the document in `render` and mutate it
only through `Document::apply`, which runs a command, recompiles for
validation problems, discards results, and notifies observers.

Every user command is a GPUI action registered at application level, so the
menu bar, toolbar, key bindings, and panel buttons dispatch the same thing.
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

- Drawing by clicking in the view. Frames and shells are created from
  selected nodes; nodes come from a dialog.
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
  orbit, and file open/save have not been exercised interactively yet and are
  not covered by automated UI tests. GPUI Kit's `test-support` feature offers
  headless UI tests and is the intended route.
