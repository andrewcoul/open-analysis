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

The solver is SI internally. The model layer is also SI internally. Display
units are a preference stored in metadata and applied only at the UI boundary.
The model layer does not convert.

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
serves both.

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
| M3 Libraries | Partial | Loader, provenance on copy, and a starter file with five sections and four materials. Full AISC and Eurocode tables are not bundled; they need a data import step and a decision about source and licence. |
| M4 Groups | Done | Groups hold any entity kind; removal strips membership and the inverse restores it; `Compiled::group_indices` maps a group to solver indices. |
| M5 Bindings | Done | `apply_commands_json`, `compile_json`, `solve_json` exposed through wasm-bindgen. PyO3 not extended, by decision. |
| M6 Agent interface | Done | `crates/oa-mcp`: stdio MCP server over a transport-independent `Session`. 22 tools covering describe, list, get, find, ids, command reference, atomic apply, undo, redo, new, load, save, library, compile, analyze, envelope, group envelope, drift, read-only SQL, and index translation. The acceptance scenario, a two-storey frame built from commands through to governing drift, runs as a test against the session. |

Decisions made during implementation:

- **`next_id` is monotonic and undo does not rewind it.** Ids are never
  reused even after undo, which keeps journals and external references safe.
  The property test masks this one field when comparing to the start state.
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
- **Rigid end offsets and cardinal points** were deferred, as the open
  question allowed.

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
- **Section orientation and offsets.** Deferred. The solver has roll and
  local-y hints but no cardinal points or rigid end offsets. End offsets
  likely need solver support and should be designed there first.
- **Result storage.** Resolved: Parquet files queried through DuckDB, owned by
  the solver crate as Phase 8. The model layer keys them by content hash and
  maps rows back to entity ids.
