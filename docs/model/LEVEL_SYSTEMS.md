# Level systems: ETABS, ADAPT-Builder, and an implementation path

Research and implementation handoff, 2026-09-17. Code inspected at commit
`ab0a12f`. Three GPT-5.6 Luna subagents, using high reasoning, researched
ETABS, ADAPT-Builder, and the repository. The parent agent checked the key
sources and code and authored this brief. This document proposes work; it
does not implement it.

Status: stages 1 to 3 below were implemented on 2026-09-17 in `oa-model`
(`levels.rs`, format version 2), `oa-mcp`, and `oa-gui` (`levels.rs`,
the viewport, properties, and the add-node dialog). Stage 4 remains.

Decisions recorded 2026-09-17 after review: every node binds to a level,
with no unbound nodes and no inference of levels from coordinates; Z is
the only structural vertical axis, with X and Y as the plan axes; the GUI
stages below stand as written.

**Recommendation:** introduce named levels and mandatory node-to-level
bindings in `oa-model`, then build active-level drawing and plan filtering
on top. Keep the existing absolute node coordinates and solver interface.
Implement elevation changes as explicit, atomic model commands. Add copying
between levels after those foundations work; defer similar-story editing,
multiple towers, and single-floor analysis.

The vendor sections describe documented behavior, not proprietary storage
internals. CSI's online help is not consistently version-labelled. ADAPT
geometry details below come from the official 2020 manuals, supplemented
by online analysis help. These are references for behavior, not a claim of
complete parity with every current release.

## 1. The concepts to keep separate

| Concept | Meaning for this implementation |
|---|---|
| Level | A named horizontal datum at an elevation, with stable identity. |
| Story | The vertical interval between successive levels; label it by its upper level when reporting “height below.” |
| Working plane | The plane where drawing input is placed. Usually the active level, but eventually it can be an arbitrary plane. |
| Geometry binding | A node's explicit relationship to a level, determining whether that node follows an elevation change. |
| Display membership | Which objects a plan or level-context view shows. An object can appear in more than one view. |
| Diaphragm | An analytical constraint or stiffness model, independent of level membership. |
| Analysis scope | The structural system actually solved, including its loads and boundary conditions. |

A useful level system connects these concepts while retaining their separate
meanings. A floor label alone cannot determine connectivity, diaphragm
behavior, or which supports an isolated-floor analysis needs.

## 2. How ETABS works

**Story data establishes the vertical stack.** Each story has a label and
height measured to the level below. The base elevation is editable; ETABS
calculates the other elevations from the heights. Its base height is zero.
The same table defines master stories and which stories are similar to a
master. ETABS also distinguishes inserting a story while preserving existing
heights from inserting one while preserving existing elevations. These are
different geometry operations and deserve explicit choices in any editor.
[CSI: Story Data Form](https://docs.csiamerica.com/help-files/etabs/Menus/File/New_Model/Story_Data_Form.htm)

**The active story and the editing scope are separate.** One Story limits
an action to the displayed story. Similar Stories extends drawing,
selection, and assignment to related stories. All Stories extends applicable
actions across the stack, excluding the base. For selection and assignment,
CSI describes matching objects of the same type at the same plan location.
This documents operation propagation; it does not establish a permanent
shared-object or parametric-instance model. An implementation should not
assume that editing one copy always updates every other copy.
[CSI: Similar Stories](https://docs.csiamerica.com/help-files/etabs/Keyboard_Commands_and_Special_Features/Similar_Stories_Drop_Down_List.htm)

**Geometry can be relative to a story.** ETABS spreadsheet geometry uses
story-relative vertical distances and a separate flag for an endpoint on
the level below. That flag lets a pasted column or wall span the destination
story even when its height differs from the source. The documented `DZ`
convention is positive downward. This is evidence for preserving endpoint
relationships during copying, rather than applying one vertical translation
to every coordinate.
[CSI: Editing Geometry Using a Spreadsheet](https://docs.csiamerica.com/help-files/etabs/Keyboard_Commands_and_Special_Features/Editing_ETABS_Geometry_Using_a_Spreadsheet.htm)

**Plan views are more than a camera orientation.** ETABS lets a plan target
a story, a reference plane, or an arbitrary global Z elevation. It shows
horizontal objects at that plane and applicable column/wall intersections.
Its documented plan-view rules exclude braces and ramps. Those are product
choices, not universal geometric rules; OA's generic frames and shells do
not currently distinguish all these physical object types.
[CSI: Set Plan View](https://docs.csiamerica.com/help-files/etabs/Menus/View/Set_Plan_View.htm)

**Reference planes supplement the story stack.** They support snapping and
plan views at intermediate elevations, including mezzanines. Their presence
does not automatically split every vertical object. CSI documents different
creation behavior when drawing a vertical object on a reference plane versus
drawing it on a story level. This is a useful distinction for a later OA
working-plane feature.
[CSI: Reference Planes](https://docs.csiamerica.com/help-files/etabs/Menus/Edit/Edit_Stories_and_Grid_Systems/Reference_Planes_Form.htm)

**Editing connected geometry has specific semantics.** Moving a joint
resizes or reorients its attached elements. Moving a frame or shell can
instead create new joints and leave neighboring objects in place. CSI also
documents restrictions on moves across story boundaries. These rules do
not establish how every possible object behaves during a story-height edit;
OA must define its own behavior explicitly.
[CSI: Move Joints/Frames/Shells](https://docs.csiamerica.com/help-files/etabs/Menus/Edit/Move_Joints_Frames_Shells.htm)

Two further distinctions matter. ETABS clipboard copying carries geometry
and property names, but excludes assignments such as loads and diaphragms;
“copy floor” therefore needs a precise contract. ETABS also supports towers
with distinct story/grid definitions and assigns each object to one tower.
Neither feature is necessary for OA's first level implementation.
[CSI: Copy and Paste](https://docs.csiamerica.com/help-files/etabs/Menus/Edit/Cut_Copy_and_Paste.htm),
[CSI: Towers](https://docs.csiamerica.com/help-files/etabs/Menus/Edit/Edit_Towers_Stories_and_Grid_Systems/Edit_Towers_Stories_and_Grid_Systems.htm)

Diaphragms remain separate named definitions assigned to objects. Their
rigid/semi-rigid settings change analysis behavior; a story definition by
itself does not establish such a constraint.
[CSI: Diaphragms](https://docs.csiamerica.com/help-files/etabs/Menus/Define/Diaphragms.htm)

## 3. How ADAPT-Builder works

**The current floor has upper and lower context.** ADAPT describes a
Current Plane with a Bottom Plane and Top Plane around it. Lower supports
normally extend to the plane below; upper supports extend to the plane
above. Level Assignment uses typical slab-top-to-slab-top distances.
Component offsets adjust this default positioning. For beams, the manual
defines zero offset at the beam top, positive downward, and negative upward.
The active level has a selector and up/down navigation.
[ADAPT-Modeler 20 manual, sections 4.2.2, 5.3–5.4, 5.6.2](https://help.risa.com/risahelp/adaptbuilder/Content/PDFs/ADAPT-Modeler%2020%20User%20Manual.pdf)

**A reference elevation is not automatically an analytical node elevation.**
Floor Pro describes floor geometry by distances from component tops to the
Current Plane. Offsets represent stepped slabs and shortened supports.
It also documents an explicit connectivity operation that can move the top
of a lower column to the slab soffit, removing the initial overlap. This
distinguishes a physical floor datum, a component's dimensions, and the
geometry ultimately used to connect components.
[ADAPT-Floor Pro 20 manual, sections 4.1.1–4.1.2](https://help.risa.com/risahelp/adaptbuilder/Content/PDFs/ADAPT-Floor%20Pro%2020%20User%20Manual.pdf)

**Single-Level and Multi-Level are actual operating modes.** In Single-Level
mode, modeling acts on the active level and analysis concerns that floor
system. In Multi-Level mode, the whole structure is displayed and analyzed,
while modeling still references the active plane. A full-building view does
not remove the need for an active drawing level.
[ADAPT-Modeler 20 manual, section 4.2.2](https://help.risa.com/risahelp/adaptbuilder/Content/PDFs/ADAPT-Modeler%2020%20User%20Manual.pdf)

The single-floor solution has its own support assumptions and stabilization
options. ADAPT's analysis help describes fixed-roller conditions allowing
horizontal shortening and optional automatic in-plane stabilization.
Consequently, analyzing an isolated level is a structural modeling feature,
not simply hiding other floors before invoking the solver.
[ADAPT: Analysis Options](https://help.risa.com/risahelp/adaptbuilder/Content/Analysis/FEM%20Analysis/analysis-options-dialog.htm)

ADAPT can also reuse column/wall reactions from multi-level analyses in a
single-level run, organized by Usage Case. Its help describes separate
retention rules for the last single- and multi-level solutions and stored
global reactions. A future OA equivalent would need explicit result
provenance and reaction-transfer rules.
[ADAPT: General FEM Analysis](https://help.risa.com/risahelp/adaptbuilder/Content/Analysis/FEM%20Analysis/general-FEM-analysis.htm)

**Copying has a source and destination plane.** RISA's modeling guide
explains that full-structure copies reference the active plane at copy time
and are placed relative to the active plane at paste time. It documents
truncation when the destination lacks enough reference planes; missing
planes are not created automatically. For OA, a better initial contract is
to reject an incomplete destination mapping and report the missing levels.
[RISA: ADAPT-Builder Modeling Tips, September 2020, pp. 8–14](https://risa.com/assets/documentation/ADAPT-Builder-Tips-and-Tricks-in-Modeling.pdf)

The useful combination for OA is ETABS-style story navigation and explicit
editing scopes, plus ADAPT-style visibility of the floor and supports above
and below. Physical slab-top placement and floor-isolation analysis require
additional machinery beyond the current analytical model.

## 4. What the repository already provides

Paths below are relative to the repository root; line anchors describe
`ab0a12f` and will move as implementation proceeds.

| Area | Existing code and implication |
|---|---|
| Entities | `crates/oa-model/src/entity.rs:24` defines `Node.position: [Length; 3]`. Frames reference two node IDs; shells reference four. There is no level binding or separate physical slab/column representation. |
| Identity and lookup | `crates/oa-model/src/model.rs` has nine `EntityKind`s and deterministic `BTreeMap` tables. Levels should join the existing global ID space. Update `all_ids`, `kind_of`, `name_of`, `referrers`, defaults, and entity dispatch. |
| Editing | `crates/oa-model/src/command.rs` returns inverse commands and supports rollback in `Batch`; `editor.rs` adds undo/redo and optional journaling. All level geometry changes belong here. |
| Files | `crates/oa-model/src/format.rs:9` is format version 1, with a migration hook and integrity checks. Existing structs deny unknown fields. |
| Compilation | `crates/oa-model/src/compile.rs:152` copies node positions into solver input and maps IDs to indices. Levels need validation but no solver index. `Compiled::content_hash` hashes the compiled solver model. |
| Units | `crates/oa-model/src/units.rs` supplies `Role::Length` and `MapQuantities`. Model files and shared JSON APIs use SI; GUI/MCP values are converted at their boundaries. |
| Drawing | `crates/oa-gui/src/viewport.rs:377` unprojects onto an axis-dependent plane at zero, then rounds all coordinates to a fixed 1 ft grid. `camera.rs:175` already accepts an arbitrary axis/value plane. |
| Rendering and picking | `viewport.rs:498`, `build_scene`, projects the model and populates pick snapshots. Both drawing and pick candidates need the same level filtering policy. |
| GUI state | `document.rs` owns model history, selection, revision, validation, and results. `workspace.rs`, `actions.rs`, and `main.rs` implement command routing. `explorer.rs` and `properties.rs` dispatch by entity kind. |
| Agent interface | `crates/oa-mcp/src/lib.rs` implements describe/list/get/find/apply and a static `COMMAND_REFERENCE`; `main.rs` defines tool arguments and descriptions. Update all relevant surfaces. |
| Other clients | `crates/oa-wasm/src/lib.rs` exposes model JSON commands/compilation. `oa-py` currently exposes the solver, not the editable model layer. |
| Results | `oa-results::ResultStore::envelope_drift` operates on node pairs. There is no level-wide drift aggregation. Diaphragms already exist independently, including synthetic masters in compilation. |

The GUI plan explicitly lists stories and grids as missing
(`docs/gui/PLAN.md:146`). Treat implementation code as authoritative where
older planning text differs from it.

## 5. Recommended first data model

Use one Z-up building-level stack. Z is the structural vertical axis and
X and Y are the plan axes; this is a model rule, not a display preference.
The camera's Y-up display toggle is a viewing convenience only and must not
affect placement, levels, or results. Removing it is reasonable follow-up
work but not part of this feature.

Conceptual schema, not a ready-to-apply patch:

```rust
struct Level {
    name: String,
    elevation: Length, // authoritative absolute global Z, stored in metres
}

// New table:
Model.levels: BTreeMap<EntityId, Level>

// New required field; every node binds to exactly one level:
Node.level: EntityId

// Derived, never a second stored coordinate:
node_offset_z = node.position[2] - levels[node.level].elevation
height_below = level.elevation - previous_level.elevation
```

**Keep absolute positions authoritative.** The level binding tells the
editor which datum the node follows. Display the offset as a derived
value; editing it writes `position[2] = elevation + offset`. Changing an
absolute node Z while retaining its binding changes its offset. Rebinding
defaults to preserving world position; “place on level” is a separate
operation that sets zero offset.

This minimizes disruption to property editing, rendering, load geometry,
compilation, and existing JSON clients. A fully relative coordinate enum
would also work, but would require replacing every direct read of
`Node.position` with a resolver. Do not add stored absolute Z and stored
offset as independently editable truths.

**A model always has at least one level.** A new model starts with one
level at elevation zero, named by the creating client. A node cannot be
added without a level, and a level cannot be removed while nodes bind to
it. Every node is therefore either on a reference plane (zero offset) or
tied to one by an offset.

**Store elevations; derive heights.** This is an OA design choice, not a
claim about ETABS internals. It fits existing absolute geometry and makes
insertion of an empty datum straightforward. Offer height editing as a
command that changes the appropriate elevations. Sort by elevation for
navigation; `BTreeMap` ID order is not floor order. Require finite elevations
and distinct ordered levels under a documented SI tolerance. Support
negative elevations and a nonzero base. The lowest level has no “height
below”; it need not be named Base.

**Use one node binding, not per-member copies of the same endpoint.** A
column is still a frame between two nodes; its endpoints may bind to
different levels. Adjacent beams share those same node IDs. A wall shell
may have corners bound to two levels. Shared nodes move once, so connected
members remain connected. Long columns may span multiple levels; inserting
an intermediate datum does not split their elements.

For the first release, derive floor membership from node bindings and
geometry. A frame/shell whose nodes all bind to one level belongs to that
level's floor view, including nonzero offsets. Mixed-level elements are
spanning elements. Add explicit object ownership later if reporting or
physical modeling needs it; ownership must not override node geometry.

**Define OA offsets as positive upward:** `z = elevation + offset_z`.
That matches global coordinates and is deliberately different from the
downward-positive ETABS clipboard and ADAPT beam conventions. Any future
import adapter must convert signs explicitly.

**Do not interpret this datum as automatic top of slab.** OA has analytical
shell coordinates and a thickness, without ADAPT's physical-component
placement/connectivity layer. A user may name a datum for a slab top, but
the editor must not silently shift the shell by half its thickness or move
column ends to the soffit. Such behavior needs an explicit physical-to-
analytical mapping and treatment of eccentric connections.

## 6. Command behavior to settle before coding

The command names are suggestions. Their semantics are the important part.

| Operation | Recommended behavior |
|---|---|
| Add level | Insert an empty datum; do not copy, split, merge, or move existing geometry. |
| Rename level | Preserve its ID and all bindings. |
| Bind/rebind nodes | Preserve world coordinates by default; derive new offsets. Reject missing/wrong-kind level IDs. |
| Place nodes on level | Explicitly set their Z to the level elevation and bind them. |
| Set level elevation, `this_level` | Move that datum and its bound nodes by the same delta. Hold other datums fixed. |
| Set level elevation, `this_and_above` | Move that datum and all higher datums, plus their bound nodes, by the same delta. |
| Set height below | Resolve the new elevation relative to the previous level; default to `this_and_above`, preserving higher story heights. |
| Remove level | Refuse while any node binds to it. Offer explicit rebind-to-another-level-then-remove as one transaction, preserving world coordinates. No implicit structural deletion. |

For either elevation-edit scope, nodes bound to other levels stay fixed. A coordinate
close to a level is not authorization to move it. Reject edits that cross
neighboring level order or produce coincident datums. Preview the affected
levels, bound nodes, and connected elements before committing a bulk move.

Example, in display feet:

```text
                 Before       Move L1 only +2       Move L1 and above +2
Roof datum       24           24                    26
L1 datum         12           14                    14
Base datum        0            0                     0
L1-bound node    11.5         13.5                  13.5
Base-bound node  11.5         11.5                  11.5
```

The L1-bound node keeps its -0.5 ft offset and the Base-bound node keeps
its +11.5 ft offset. With Base/L1/Roof-bound column
endpoints, moving L1 alone lengthens the lower story and shortens the upper
one; moving L1 and above preserves the upper story height.

Implement the operation in `oa-model`, preferably using a small pure
`levels.rs` module for ordering, membership, and edit planning. Stage a
complete proposed change, validate it, then commit through one command.
Store exact previous level/node values for undo; repeated subtraction of
deltas is not an exact inverse. Ensure redo and journal replay use the same
semantics. A generic level update must not bypass the documented geometry
policy or move nodes a second time while restoring an inverse.

Existing `Batch` provides rollback when a command fails, but ordinary
entity updates do not by themselves validate every geometric consequence.
Level moves need checks for affected zero-length frames, invalid/warped
shells, invalid diaphragm geometry, and member-load stations outside a
changed member length. Preserve existing member-load distances by default;
do not silently scale loads or their stations. Report affected loads and
reject newly invalid moves. Do not require an unrelated, unfinished model
to be fully solvable just to edit its levels.

Use a small documented geometric tolerance in metres, independently of
the drawing snap interval. Never merge coincident nodes merely because
they share a level: coincident but disconnected nodes can be intentional.

## 7. GUI integration

Add **Define > Levels** using the existing dialog/table patterns, with
name, elevation, and clearly labelled height-below columns. Elevation and
height edits invoke the commands above. Add an active-level selector and
up/down actions near the view controls, and show the level/elevation in the
prompt or status bar. Expose node binding and offset in Properties.

Keep active level, view mode, context visibility, and working-plane offset
as transient `Workspace`/`Viewport` state initially. Changing them must not
mark the document dirty or discard analysis. On open, undo, or deletion,
resolve the active ID against the current model; choose a deterministic
remaining level or clear it. Do not store an active level by list index.

Three useful display modes are:

| Mode | Display and interaction |
|---|---|
| Whole model | Existing 3D behavior; the active level still determines new node placement. |
| Active level | Its bound floor objects and node markers, plus clearly differentiated intersections of spanning objects. |
| Active level with context | Add the story above/below as subdued context; context is not editable or snappable by default. |

Use one visibility/classification result for painting, labels, picking,
snapping, deformed geometry, force diagrams, and fit-to-view. In an active
plan, exclude hidden floors from pick snapshots. Clear hidden geometric
selection and partial drawing picks when switching active levels so an
off-screen entity cannot receive an accidental assignment. Whole-model
selection can remain an explicit separate action.

Display membership is not just `abs(node.z - elevation) < tolerance`.
That would hide offset floor objects and mishandle columns/walls. Derive
floor objects from bindings, and intersect spanning geometry with the
active plane for support context. Objects bound to other levels that merely
lie in the plane are not floor objects of the active level; viewing must
never change a binding. Distinguish
an intersection marker from a real node: snapping to it cannot invent
structural connectivity. Provide whole-model view for ramps and other
mixed-level geometry whose plan representation is ambiguous.

Replace `snap_to_ground` with explicit work-plane placement. The current
`Camera::unproject` can intersect `Some((2, active_elevation))`. Snap X and Y,
then retain the exact requested Z; the existing rounding of all three
coordinates would corrupt a level at, for example, 12.5 ft. Extend the
placement event to carry the intended level binding as well as position.
Frame/shell tools can continue using node IDs.

A model always has at least one level, so the Node tool always has an
active level to place on. Selecting a level must not retroactively rebind
existing geometry.

Active-level plan should look down global Z even if the general 3D viewer
was using Y-up. An edge-on elevation view needs an explicit vertical working
plane or disabled free placement; do not silently treat the camera-target
fallback as a valid placement on the active level. Camera orientation must
never redefine structural vertical.

## 8. Copying and similar levels: a later increment

Implement explicit **Copy selected geometry to levels** before persistent
similar-story behavior. It needs a source level, destination levels, an
endpoint-level mapping, and a complete preview.

For a floor object, map source-level bindings to the destination and
preserve offsets. For a column/wall spanning the level below, map both
end levels and preserve each endpoint offset. A translation alone cannot
adapt a column from a 12 ft story to a 15 ft story. Require explicit mappings
for longer spans and reject missing destination levels.

Allocate fresh IDs, generate unique names, and remap internal references
once per copied node. Reuse material/section definitions. Specify the
handling of nodal restraints, springs, masses, releases, loads, groups,
and diaphragms individually. A reasonable first contract copies geometry
and member properties, while loads, nodal support/mass assignments, and
diaphragms require explicit options. Reuse an existing destination node
only under an explicit, compatible connectivity policy; otherwise report
the collision. Reject duplicate members by default. Apply the full copy
as one undoable transaction.

Similar-level editing can later resolve an explicit list of target levels
and corresponding object IDs, then issue the same commands. Show that list
and missing/ambiguous matches. A master/similar relationship should not
make separate objects share IDs or create silent persistent synchronization.

## 9. Analysis, compatibility, and easily missed integration work

**Keep analysis independent of visibility.** All existing analysis commands
continue solving the full compiled model. New UI controls should say
“Active level view,” not imply a single-level analysis mode. A future
isolated-floor solver path needs extraction, support assumptions, reaction
transfer, and result identity including scope and source model.

**Keep diaphragms explicit.** Level creation/binding must not create or
change constraints. A level can have no diaphragm or multiple independent
diaphragms. Existing rigid diaphragm master/slave behavior remains intact.

**Make Z the vertical axis everywhere.** In
`crates/oa-mcp/src/lib.rs:531`, `Session::drift` uses `position[1]` for
height, and the MCP acceptance fixture in `crates/oa-mcp/tests/session.rs`
is built Y-up, while the GUI example in `crates/oa-gui/src/document.rs:327`
is Z-up. Vertically aligned nodes in the Z-up model therefore produce no
ratio under the existing calculation. Change `drift` to use `position[2]`
and convert the MCP fixture to Z-up in stage 1. There is no Y-up
compatibility path. Do not infer the axis from gravity, displacement
component, or camera orientation. State whether a reported denominator is
actual node separation or datum-to-datum height. Whole-story maximum drift
also requires corresponding node pairs and comparison within the same
load combination; subtracting independent displacement envelopes is wrong.

**Migrate files without inventing floors.** Bump the model format to 2.
A v2 model always has at least one level and every node binds to one.
Migrate v1 by creating a single level at elevation zero and binding every
node to it with its existing Z as the offset. This preserves coordinates
and compiled solver content exactly and invents no floors; the user then
adds levels and rebinds nodes explicitly. Keep the v1 fixture and add v2
coverage. Update format integrity checks and compile validation for level
IDs, names, bindings, and finite elevations. `Model::from_solver` applies
the same single-level rule. Older v1 builds already reject the new format
through the existing `TooNew` check.

**Update every interface, not just the new struct.** Extend `Entity`
references, `Model::referrers` scans, exhaustive entity-kind matches,
GUI deletion routing, explorer/properties, MCP counts/list/get/find,
tool descriptions, and `COMMAND_REFERENCE`. `add_node` and node JSON
require `level`; there is no default. Level removal must not fall
through the node-delete cascade and destroy structural members. Include
node-to-level references in compile validation: the current compiler checks
references for frames/shells and other entities, but does not call
`check_references::<Node>` because nodes currently reference nothing. Include
levels and all new dimensional command arguments in `MapQuantities`,
including preview outputs. MCP feet and shared JSON API metres must remain
distinct. WASM should obtain the commands through the existing JSON path;
a new Python model API is separate scope.

**Preserve result freshness rules.** GUI and MCP currently invalidate
results after every model edit; retain that conservative behavior for v1
of this feature. A geometry move changes the compiled solver hash. A
level rename or binding-only edit can leave it unchanged. This is expected:
solver results depend on compiled geometry, while level-based report
membership must be recomputed from the current document. View changes are
not model edits and should preserve results.

## 10. Suggested implementation sequence and acceptance checks

1. **Model foundation:** Level entity/table, node bindings, units, validation,
   commands, migration, and JSON/MCP exposure. Put shared logic in `oa-model`.
2. **Useful level workflow:** Levels dialog, active selector/navigation,
   exact-elevation drawing, node properties, filtered display and picking.
   Add levels and bindings to the GUI example through the normal model API.
3. **Geometry editing:** Transactional elevation/height operations, impact
   preview, offset preservation, affected-element validation, undo/replay.
4. **Repetition:** Explicit copying with full reference remapping and clear
   assignment options. Similar-level scopes can follow separately.

Stages 1–3 form the recommended first release. The handoff should be
considered implemented only when these behaviors are demonstrated:

| Scenario | Expected result |
|---|---|
| Load existing v1 fixture | One level at elevation zero with every node bound to it by its Z offset; identical compiled geometry and analysis input. |
| New level identity | IDs remain globally unique through deletion, undo, redo, save/load, and journaling. |
| Invalid definition | Blank/duplicate names, nonfinite elevations, invalid order, and dangling bindings fail with named errors. |
| Coincident nodes bound to different levels | Only the node bound to the moved level follows the move. |
| Offset floor and shared column node | Offsets and XY stay fixed; shared nodes move once; connected elements retain node IDs. |
| Two movement scopes | Reproduce the 0/12/24 ft example above; preserve the intended higher story heights. |
| Invalid geometric consequence | A newly invalid frame/shell/diaphragm or out-of-span member load rejects the whole move. |
| Undo and failed batch | Restore exact prior level/node values and bindings; replay produces the same accepted state. Respect the existing non-reused ID policy. |
| 12.5 ft active plane | Placement preserves 12.5 ft exactly within conversion precision; no whole-foot Z snapping. |
| Two nodes at the same plan XY | Only the active/selectable floor node can be picked; context cannot steal a snap. |
| Plan supports and offsets | Offset floor objects remain visible; spanning-member intersections do not create nodes. |
| Change view versus change geometry | Switching levels preserves current results; a model edit invalidates them. |
| Drift axis | Height is taken from Z; the converted MCP fixture yields a verified dimensionless ratio. |
| Unit boundary | Equivalent feet-based MCP commands and metre-based model JSON produce equivalent SI documents. |

Extend `crates/oa-model/tests/model.rs` and `crates/oa-mcp/tests/session.rs`;
put pure visibility and work-plane arithmetic tests beside the GUI helpers.
Run targeted model/MCP/GUI tests, compile the affected workspace crates,
and check the model's no-default-features configuration used by bindings.
Use the repository's `scripts/cargo.ps1` wrapper where required. Manually
exercise the GUI example for selection, navigation, drawing, property edits,
save/reopen, and undo. No runtime tests were run for this documentation-only
research task.

Suggested prompt for the implementing agent:

> Read `docs/model/LEVEL_SYSTEMS.md` and the current model/GUI code. Implement
> stages 1–3 as the first release: named Z-elevation levels, mandatory node
> bindings with authoritative absolute coordinates, safe atomic level edits,
> versioned persistence, GUI plan/work-plane behavior, and MCP access. Follow
> the acceptance table, migrate v1 files to a single base level, use Z as
> the only vertical axis, and keep all existing solves full-model. Treat copy/similar-story features, towers,
> physical slab-top offsets, and isolated-floor analysis as later work.
