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

### Python and WebAssembly

The model layer is exposed through the same JSON approach as the solver. A
`compile_json` entry point takes a model document and returns the solver
request plus the mapping. Python and browser clients build models using
commands serialized as JSON, so there is one code path for editing.

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

- `compile_json` and command application through PyO3 and wasm-bindgen.
- Example script that builds a model through commands and analyses it.

## Open questions

- **Automatic diaphragm masters.** The solver requires an explicit master node.
  ETABS places one at the centre of mass automatically. The model layer should
  probably create and manage that node, updating its position as masses change.
  Decide in M0.
- **Section orientation and offsets.** The solver has roll and local-y hints
  but no cardinal points or rigid end offsets. These are model-layer concepts
  that compile to node positions and releases, but end offsets may need solver
  support. Decide in M0.
- **Result storage.** Keep results in memory keyed by model hash for now. A
  disk-backed result store is deferred, matching the solver plan.
