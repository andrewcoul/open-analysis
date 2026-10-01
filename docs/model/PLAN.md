# Model Layer Plan

Plan for `oa-model`, the editable structural model that sits between a user
interface and the solver. Written 2026-09-14, before any code exists.

## Why a separate layer

The solver in `oa-core` takes flat tables indexed by position. Node 5 is the
sixth entry in the array. That is the right shape for a solver: it is fast, it
parallelizes, and it crosses the Python and WebAssembly boundary cheaply. It is
the wrong shape for an editable model. Delete a node and every frame that
references a later node is now wrong. There are no names, no groups, no section
library, and no undo.

`oa-model` owns the editable representation and compiles it down to solver
input. The solver crate does not change. The model layer is a client of it.

## Responsibilities

| Concern | Owner |
|---|---|
| Stable identity, names, groups, libraries, file format, edit history | `oa-model` |
| Element formulations, assembly, factorization, analysis, results | `oa-core` |
| Mapping between the two, in both directions | `oa-model` |

The model layer never does structural arithmetic. If a feature needs stiffness
or mass, it belongs in the solver and the model layer exposes it.

## Design

### Stable identity

Every entity gets an `EntityId`, a 64-bit integer allocated once and never
reused within a model. Nodes, frames, shells, materials, sections, diaphragms,
load cases, and combinations all use the same id space so an id alone says
what it refers to.

Ids survive deletion, reordering, undo, and file round-trips. Solver indices
are an output of compilation, not a property of the entity.

### Names and groups

Every entity has an optional user-visible name. Names are unique per entity
kind. Groups are named sets of entity ids, and an entity may belong to many
groups. Selection, load assignment, and reporting work on groups, so a user can
say "Level 3 columns" without knowing indices.

### Libraries

Section and material tables ship as data, not code:

- AISC shapes with the properties the solver needs plus the ones design checks
  will need later (Zx, Sx, rx, ry, J, Cw).
- Eurocode IPE, HEA, HEB, and hollow sections.
- Standard materials: structural steels, concrete grades, timber.

A library entry is copied into the model on use, so a saved file is
self-contained and does not change when the library is updated. The copy
records which library and version it came from.

### Model definition

```
Model
  metadata        name, units preference for display, created, modified
  gravity
  nodes           EntityId -> Node
  materials       EntityId -> Material
  sections        EntityId -> Section
  frames          EntityId -> Frame       (references nodes, material, section)
  shells          EntityId -> Shell       (references nodes, material)
  diaphragms      EntityId -> Diaphragm   (references master and slave nodes)
  load_cases      EntityId -> LoadCase    (loads reference nodes, frames, shells)
  combinations    EntityId -> Combination (references load cases)
  groups          EntityId -> Group       (references any entity)
```

Each table is a `BTreeMap<EntityId, T>` so iteration order is deterministic.
Entity types mirror the solver's types but hold `EntityId` references instead of
indices, and carry the display name and library provenance.

### Units

The solver is SI internally. The model layer is also SI internally, and so is
the saved file. Display units live in `units.rs`: a `Role` names what a
number means at the boundary (coordinate, thickness, force, stress, and so
on, since US practice mixes feet and inches within one dimension), and a
`UnitSystem` gives each role a symbol and a factor. `MapQuantities` rescales
every quantity in an entity or a command, so the GUI, the MCP server, and
the library data all convert through the same table. US customary (kip, ft,
in) is the only system built; SI is a second table, selected by the
`display_units` preference in metadata once it exists. A library file states
its own units so tables can be typed in as published.

### Compilation

`compile(&Model) -> Result<Compiled>` produces:

- A solver `oa_core::Model` with dense, zero-based indices.
- A `Mapping` with `EntityId -> index` and `index -> EntityId` for every kind.
- A list of validation problems tied to entity ids, so the UI can highlight
  the offending node rather than report "frame 412".

Compilation is pure and cheap enough to run on every edit. Results from the
solver are re-attached to entities through the mapping.

### The command API is the primary interface

The model layer is designed to be driven by AI agents as a first-class
client, alongside a GUI. That decision, made 2026-09-14, shapes several
things that would otherwise be conveniences:

- **Names are the addressing scheme.** An agent says "the Level 4 beams",
  not "frames 380 to 421". Every entity has a name, names are required
  rather than optional for anything an agent can create, and groups are how
  selections are expressed.
- **Errors name entities.** Validation problems carry entity names and ids,
  never solver indices, so an agent can act on them.
- **Commands are the tool surface.** Each command has a JSON schema and a
  one-line description. That is what an agent sees, so the command set is
  designed to read well as a list of tools, not just to be complete.
- **The GUI is one client.** It issues the same commands an agent does. No
  edit path bypasses the command interface.

### Edit operations

All changes go through a command interface:

```
enum Command {
    AddNode { id, node },
    RemoveNode { id },
    SetNodeRestraint { id, restrained },
    AddFrame { id, frame },
    ...
}
fn apply(&mut self, cmd: Command) -> Result<Command>   // returns the inverse
```

Every command returns its inverse, so undo and redo are a stack of commands and
nothing else. A GUI, a script, and a file import all issue the same commands.
Removing an entity that others reference is refused unless the command
cascades, and the cascade is explicit in the command.

### File format

JSON, one document per model, with a top-level `format_version`. Each version
bump ships a migration function from the previous version. Migrations are
tested with a fixture file per version that must load and compile after
migrating.

Entity ids are written as integers. The file is self-contained: library
entries are copied in, not referenced.

Results are not stored in the model file. They are a separate artifact keyed by
model content hash, so a stale result cannot be mistaken for a current one.

### Storage: why not a database for the model

SQLite and similar stores were considered for the model and rejected. An
editable model is small by database standards, loads into memory in well under
a second even at a few hundred thousand entities, and every operation the model
layer performs wants typed structs with direct references. A database under
that would mean either round-tripping every edit or maintaining two copies of
the truth, and it would make the file diff badly in version control. ETABS
makes the same choice: its `.EDB` is a document loaded whole into memory, with
a text mirror for recovery.

So the model is in-memory structs while editing and versioned JSON on disk.

Results are the opposite case and get a real store: SQLite through
`rusqlite`, designed in the solver plan's Phase 8. The model layer's
obligations toward it are:

- Write the model content hash into every result store it triggers.
- Refuse to attach results whose hash does not match the current model.
- Map result rows back to entity ids through the compilation mapping.

The command log is journalled to a small SQLite file for autosave and crash
recovery in Phase M1. Same crate as the result store, so one dependency
serves both. Only accepted commands are journalled: the model applies a
command first and records it second, and if recording fails the edit is
rolled back, so a replay never stops at a rejected edit. Model documents are
saved through a sibling temporary file and a rename, so a failed save leaves
the previous document intact, and loading checks that ids are unique across
tables, group members exist, and the allocator is past every id.

### Python and WebAssembly

The model layer is exposed through the same JSON approach as the solver. A
`compile_json` entry point takes a model document and returns the solver
request plus the mapping. Python and browser clients build models using
commands serialized as JSON, so there is one code path for editing.

## Implementation status

As of 2026-09-14, phases M0 through M5 exist in `crates/oa-model` with 11
acceptance tests, and M6 exists in `crates/oa-mcp` with 2.

Decisions made while building M6:

- **Tools take plain ids and kind names.** The model layer's `EntityId` and
  `EntityKind` are not given JSON schemas; the server converts at the
  boundary. That keeps `schemars` out of the model crate.
- **Commands are passed as JSON, documented by a tool.** `apply_commands`
  takes an array of command objects and `command_reference` returns a
  worked example of each. A schema for the whole command enum would be
  large and less readable to an agent than the examples.
- **Any edit discards results.** The session drops its compilation and
  result store on every applied, undone, or redone command, so an agent
  can never read results from a model that no longer matches them.
- **Summaries are bounded.** `list_entities` and `group_envelope` take a
  limit and report whether they truncated; `query_results` caps rows at
  1,000.

| Phase | Status | Notes |
|---|---|---|
| M0 Entities and compilation | Done | `EntityId`, nine entity tables as `BTreeMap`, `compile` with two-way `Mapping` and entity-addressed `Problem`s. Every solver fixture round-trips through `Model::from_solver` to an identical solver model. Diaphragms with `master: None` get a synthetic master at the mass-weighted centroid; verified to give the same modal eigenvalues as an explicit master. |
| M1 Commands and history | Done | 29 command variants, each returning its inverse; `Batch` rolls back on first failure; `Editor` with undo and redo; SQLite command `Journal` with replay. Property test: 300 random commands, undo to start, redo to end. |
| M2 File format | Done | `format_version`, migration hook, `tests/fixtures/format_v1.json`. |
| M3 Libraries | Partial | Loader, provenance on copy, and a starter file with ten sections and eight materials. Starter materials carry design strengths: Fy and Fu for the steels (A992, A36, A572 Gr. 50, A500 Gr. C rectangular and round, A53 Gr. B) and f'c for the concretes. `Material` holds them as optional `fy`, `fu` and `fc`, checked positive and finite with Fu at least Fy, and the file format went to version 5 for them. The AISC Shapes Database v16.0 is bundled as `data/aisc_v16.json`: 1,523 shapes (W, M, S, HP, C, MC, WT, MT, ST, HSS, pipe) with design properties (dimensions, S, Z, r, Cw, rts, ho, slenderness ratios), regenerated by `scripts/import_aisc_shapes.ps1`. A copied section keeps them as `shape`, in SI, and the file format went to version 4 for it. A copied section also gets shear areas, `shear_y` and `shear_z`, which make the solver's frame a Timoshenko beam: d·tw along the web and 5/3·bf·tf across two flanges (5/6·bf·tf for a tee's one flange), 2·tdes·Ht and 2·tdes·B for rectangular HSS, and (0.5 + 0.8·tdes/OD)·A for round HSS and pipe (CSI's (0.9 − 0.4·s)·A with s the ratio of inner to outer radius), as ETABS and SAP2000 take them. They are optional, checked positive and finite, and the file format went to version 6 for them; a section without them is rigid in shear, as before. Single angles (inclined principal axes) and double angles (no torsion constant in the table) are left out. The GUI and `oa-mcp` pick sections from it; `oa-mcp` adds a `library_section` tool and filters `library`. Eurocode tables are not bundled. |
| M4 Groups | Done | Groups hold any entity kind; removal strips membership and the inverse restores it; `Compiled::group_indices` maps a group to solver indices. |
| M5 Bindings | Done | `apply_commands_json`, `compile_json`, `solve_json` exposed through wasm-bindgen. PyO3 not extended, by decision. |
| M6 Agent interface | Done | `crates/oa-mcp`: stdio MCP server over a transport-independent `Session`. 22 tools covering describe, list, get, find, ids, command reference, atomic apply, undo, redo, new, load, save, library, compile, analyze, envelope, group envelope, drift, read-only SQL, and index translation. The acceptance scenario, a two-storey frame built from commands through to governing drift, runs as a test against the session. |
| M7 Agent interface catch-up | Done | See [docs/mcp/PLAN.md](../mcp/PLAN.md). The command reference is tested against the command enum, and the tools, 26 in all, add ASCE 7 combinations, modal, response spectrum, and spectrum peaks. `oa-mcp --attach` joins an agent to the model open in the GUI, sharing its undo history. |

Decisions made during implementation:

- **`next_id` is monotonic and undo does not rewind it.** Ids are never
  reused even after undo, which keeps journals and external references safe.
  The property test masks this one field when comparing to the start state.
  The counter saturates rather than wrapping, and a loaded document whose
  counter lags its ids has it moved past the high-water mark.
- **Compilation checks element geometry.** Degenerate, warped, or crossed
  shells and unusable frame orientations are reported as entity-addressed
  problems at compile time, the same checks analysis applies, so the GUI's
  problem list and the agent's `compile` see them before a solve.
- **Removal is refused while referenced.** No cascade command yet. A caller
  removes dependents first, in a `Batch` if it wants atomicity.
- **Group membership is part of removal's inverse.** Removing an entity
  strips it from groups; the inverse is a `Batch` that re-adds the entity
  and restores each affected group.
- **Solver validation errors arrive without entity ids.** Reference and
  naming problems are caught by the model layer with ids. Errors from the
  solver's own validation, such as a zero-length frame, are passed through
  as a single problem with no entity. Structured errors from the solver
  would fix that and are a possible later change to `oa-core`.
- **Frames carry offsets and a cardinal point.** `offsets` is the solver's
  (see its plan): joint offsets, end length offsets and the rigid-zone
  factor, edited in inches in US units. `cardinal_point` is ETABS's
  insertion point 1 to 10, the point of the section on the line between the
  nodes, seen from end I with local y up. With joint offsets it sits on the
  line between the moved ends, in that line's axes. Compilation resolves the
  joint offsets in global axes and adds the cardinal point's shift, the
  same at both ends, so the member keeps that line's direction and axes. The
  shift comes from the section's steel shape: its depth along y and width
  along z, with a channel's centroid x-bar from its web at the left and a
  tee's y-bar below its flange at the top. A section without a shape can only be
  placed at its centroid; any other point is a problem on the frame. The
  file format went to version 9; a version 8 frame runs node to node at its
  centroid. The GUI edits both per frame.
- **Stiffness modifiers belong to members, not sections.** A frame or a
  shell carries `modifiers` (see the solver plan), as ETABS assignments do,
  because cracking follows a member's role: one W or rectangular section
  can be a beam in one place and a column in another. Add and update
  commands refuse a modifier that is not positive and finite. A shell may
  also carry `local_x`, the reference its local axes and its f11 and f22
  factors follow. The file format went to version 7 for both; a version 6
  member has full stiffness and first-edge axes. The GUI edits them per
  member, picks a shell's local x from its first edge or a global axis, and
  assigns ACI 318 presets to a selection.
- **Mass sources are named entities with a model default.** A
  `MassSource` holds `element_mass`, a list of (load case, multiplier),
  `lateral`, `vertical` and `lump_to_levels` (see the solver plan), and
  references its cases, so a case a source lists cannot be removed, as for
  combinations. `default_mass_source` names the one compilation puts in the
  solver model; None means node and element mass in every direction, which
  is CSI's default too. `SetDefaultMassSource` changes it, and the default
  source cannot be removed. `Compiled::with_mass_source` hands analysis any
  other source, so modal and spectrum runs can compare them without
  recompiling. Add and update refuse a repeated case, a multiplier that is
  not positive and finite, and a source with neither lateral nor vertical
  mass: checks on the source alone. A listed case with self-weight while
  element mass is on is left to compilation, which checks every source,
  not only the default, and names the source in a problem. That depends on
  the case, which can gain self-weight after the source accepted it, and a
  command check would refuse the inverse that undo or a batch rollback
  needs to restore such a source.
- **Lumping to levels follows the nearest level.** ETABS documents only
  that lateral mass between story levels moves to the nearest one. Here a
  node off every level sends its lateral mass to the node directly below
  or above it on the nearest level, found by position within the level
  tolerance; a node exactly halfway splits evenly, and a node above the top
  level or below the lowest goes to that level. A node with no node there,
  such as a brace midpoint, is a compile problem naming it rather than a
  guess. Levels are datums, so lumping follows geometry, not the level a
  node binds to.
- **Mass and weight modifiers sit beside the stiffness ones** on frames
  and shells, 0 allowed. The GUI's ACI presets replace only the stiffness
  factors.

The file format went to version 8 for mass sources and the two modifiers;
a version 7 document has no sources and full mass and weight. The GUI's
Define menu opens a mass sources table laid out like the combinations: a
row per source with its name and five switches (default, own mass,
lateral, vertical, lump to levels), then a multiplier column per case,
blank for a case the source leaves out. The first source added becomes the
default, removing the default clears it first, and removing a load case
takes it out of every source in the same undo step.

- **Splitting frames at nodes on their spans.** The solver joins elements
  only through shared nodes, so a node lying on a beam does nothing until
  the beam is split there. `SplitFrames { frames, nodes }` splits the listed
  frames at the listed nodes, either list empty meaning all; a node lies on
  a span when it is within `split::ON_SPAN_TOLERANCE` (0.1 mm) of the axis
  and that far inside both ends. It plans a batch of plain commands, as
  `SetLevelElevation` does, so undo restores exact values and no file
  format change is needed. The frame keeps its id and name as the first
  piece, so loads, groups and results that name it still do; the other
  pieces are new frames named `B1-2`, `B1-3`, and so on, copying material,
  section, orientation, behaviour and modifiers. I-end releases stay on
  the first piece and J-end releases on the last; the new joints are
  continuous. The frame's groups gain the new pieces, a point load moves
  to the piece it falls on (the earlier one at a joint), and a distributed
  load is cut at the joints with its intensity interpolated, so the loads
  on the pieces sum to the original. New ids run from `next_id` in frame
  order, so a journal replay produces the same ids. The GUI splits on
  every draw: a placed or typed node splits the frames it lands on, and a
  drawn frame splits at the nodes it passes over and splits the frames its
  ends land on, all in the drawing's undo step. Shell edges are not split.
  End offsets stay at the member's ends, the I one on the first piece and
  the J one on the last, and the rigid zone factor and cardinal point carry
  to every piece. Joint offsets are interpolated to the new joints, in
  global axes, so every piece lies on the original's moved line, and load
  stations, which run along that line, are cut at the same fractions.

## Phases

### Phase M0: Entities and compilation

- `EntityId`, the entity types, `Model` with `BTreeMap` tables.
- `compile` with mapping and validation problems tied to ids.
- Round-trip test: build a model, compile, solve, re-attach results.
- Differential test: every fixture in `oa-core` expressed through the model
  layer compiles to an identical solver request.

### Phase M1: Commands and history

- `Command` enum covering every edit, each returning its inverse.
- Undo and redo stack.
- Property test: any random command sequence followed by its inverses
  returns the model to its starting state.

### Phase M2: File format

- JSON serialization with `format_version`.
- Migration framework with one fixture per version.
- Round-trip test: save, load, compile, compare with the original solver
  request.

### Phase M3: Libraries

- AISC and Eurocode section tables as data files with a loader.
- Standard material table.
- Provenance recorded on copy.

### Phase M4: Groups and selection

- Named groups with membership.
- Load assignment and result queries by group.

### Phase M5: Bindings

- `compile_json` and command application through wasm-bindgen for the
  browser, and through PyO3 for the verification harness only.
- Example that builds a model through JSON commands and analyses it.

### Phase M6: Agent interface

- An MCP server crate on top of the model layer, using the official Rust
  SDK. It exposes the commands, compile and analyse, and result queries as
  tools with schemas. No Python in the path.
- **Atomic batches.** A tool that applies a list of commands and rolls back
  on the first failure, so an agent's multi-step change lands whole or not
  at all.
- **Summarisation tools.** An agent cannot read a 100,000 node model.
  Tools like describe-model, list-groups, and summarise-results-for-group
  return sizes that fit a context window.
- **Result queries.** The Phase 8 envelope and drift functions as tools, and
  a raw SQL tool over the result store with a row limit.
- Acceptance: an agent session that builds a two-storey frame from a
  description, runs it, and reports the governing drift, driven end to end
  through the MCP tools with no hand-written code.

## Open questions

- **Automatic diaphragm masters.** Resolved in M0: a diaphragm may omit its
  master, and compilation creates one at the mass-weighted centroid of the
  slaves. It is recorded in `Mapping::synthetic_masters` and has no entity id.
- **Section orientation and offsets.** Resolved: the solver takes joint
  and end offsets with a rigid-zone factor, and the model layer turns
  cardinal points into joint offsets (see Decisions).
- **Result storage.** Resolved: Parquet files queried through DuckDB, owned by
  the solver crate as Phase 8. The model layer keys them by content hash and
  maps rows back to entity ids.
