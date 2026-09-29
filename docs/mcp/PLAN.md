# Agent Interface Plan (Phase M7)

Written 2026-09-28. Phase M6 of the [model plan](../model/PLAN.md) built
`oa-mcp` against the model as it stood on 2026-09-14. Since then the model
and the GUI have gained levels, underlays, load types, and the ASCE 7
combination generator, and the agent surface has not kept up. This phase
brings it level and lets an agent work on the model that is open in the GUI.

## Scope

1. A command reference that cannot silently fall behind the command set.
2. The ASCE 7 combination generator as a tool.
3. Modal and response-spectrum analysis as tools.
4. Attaching an agent to the running GUI, so its edits appear live and share
   the user's undo history.

## 1. Command reference

`COMMAND_REFERENCE` is hand-written text. It has no example for underlays,
`set_metadata`, surface loads, or a load case's `load_type`. The ASCE 7
generator matches cases by that type, so an agent that cannot set it cannot
use the generator.

- Add the missing entries.
- A test reads every `Command` tag from serde's own "expected one of" list
  for an unknown tag, so a new variant is covered with nothing to maintain.
  It checks that the list looks right, so a change in serde's message fails
  loudly instead of passing empty. Each tag must appear in the reference. An
  `update_x` or `remove_x` counts as covered when `add_x` appears, because
  the reference states that both exist for every kind.
- The test also parses every `{"command": ...}` example in the reference as a
  `Command`, so a renamed or removed field breaks the build rather than the
  agent. An example that elides fields with `...` is skipped.

## 2. ASCE 7 combinations

`generate_combinations(edition, method)` with edition `"7-16"` or `"7-22"`
(default 7-22) and method `"strength"` or `"allowable_stress"` (default
strength). It applies the same batch as the GUI's generator dialog, so the
result is one undo step, and returns the names it added. The code that turns
generated combinations into commands moves from the dialog into
`oa_model::asce7`, so the GUI and the agent share one path.

## 3. Modal and response spectrum

Mass is the frames' and shells' own mass from material density, lumped to
their nodes, plus any nodal `mass`. Superimposed dead load in a load case is
not mass. The tool descriptions say so; otherwise an agent that models
floor weight as loads will read periods that are far too short.

- `modal(modes = 6)` returns, per mode, the period, frequency, and mass
  ratio in X, Y, Z, then the cumulative ratios, the total free mass, the
  orthogonality error, and the Sturm check. It does not return mode shapes;
  they grow with the model and an agent rarely needs them as numbers.
- `response_spectrum(spectrum, direction, damping = 0.05, combination =
  "cqc", modes = 12, minimum_mass_ratio)` takes the spectrum as
  `[period s, Sa]` pairs with Sa as a fraction of g, the way design spectra
  are written. g is the model's own gravity. It returns the modal summary,
  the captured mass ratio, and the base reaction, and keeps the peaks for
  queries. Like static results, the peaks are discarded on any edit.
- `spectrum_peaks(quantity, component, id | group, limit)` reads those peaks,
  largest first. CQC and SRSS peaks carry no sign, so they are reported as
  magnitudes. They are kept apart from the static envelopes and never mixed
  into them. There is no spectrum drift: the difference of two peaks is not
  the peak of the difference.

## 4. Attaching to the GUI

### Transport

The GUI hosts the same tools on a local socket. `oa-mcp --attach` bridges
stdio to that socket, so a client configures a stdio command exactly as it
does now:

```json
{ "mcpServers": { "open-analysis": { "command": "oa-mcp", "args": ["--attach"] } } }
```

A socket was chosen over Streamable HTTP for three reasons:

- Every MCP client supports stdio.
- A Unix socket is protected by file permissions and a named pipe by its
  ACL, so there is no port, token, or Origin check to get wrong.
- The bridge is a byte copy, so there is no second MCP implementation.

The socket is `$XDG_RUNTIME_DIR/open-analysis.sock` on Unix. Without that
variable it goes in a directory `open-analysis-<uid>` under the temp
directory, which is created with mode 0700 and refused if another user owns
it. On Windows it is the named pipe `\\.\pipe\open-analysis-<user>`.
`OA_SOCKET` overrides the path on both sides, and `OA_SOCKET=off` stops the
GUI from hosting. The first GUI process to bind wins. A socket file that
nothing answers is treated as stale and replaced.

### One session, one undo stack

`Document` owns an `oa_mcp::Session` in place of its bare `Editor` and path.
The agent's batches and the user's edits land on the same undo stack, so
Ctrl+Z undoes an agent's batch as one step.

Tool calls run on the GPUI foreground thread. The socket side runs on a
tokio runtime on a background thread and sends each call as a job over a
channel. A task on the `Document` entity runs the job and sends back the
reply. Calls are therefore serialized with the user's own edits, and the
model needs no lock. A long analysis blocks the window, as Ctrl+R does
today.

The session counts revisions. After each job, `Document` compares the
count and, if the model changed, does what a GUI edit does: marks the
document dirty, drops stale results, revalidates, prunes the selection, and
notifies the views.

Analysis is shared too. `Session::analyze` can keep an in-memory copy of
each combination next to the SQLite store, and the GUI's Run static analysis
goes through it. A run from either side makes results current for both, and
the view draws the deformed shape of an agent's run.

### Protecting the user's work

- `new_model` and `load_model` are refused while the GUI has unsaved
  changes. The user would lose work that undo cannot restore. The headless
  server keeps its current behaviour.
- An agent's `undo` or `redo` is refused when anyone else has edited since
  the agent's own last change, or when the step it would act on is not its
  own. The session records who made each entry on the undo and redo stacks,
  so an agent that has undone all its own changes stops at the user's.
  Otherwise an agent that means to take back its own edit could silently
  undo the user's. The refusal says to use Ctrl+Z in the GUI.
- Each connection is its own agent, so one agent cannot undo another's
  change either.
- When attached, the server's instructions tell the agent that a person is
  watching the model and shares its undo history.

The status bar shows whether an agent is connected. Help > About gives the
configuration line.

Known limit: on Windows another user could create the pipe name first. The
GUI then fails to host, since it asks for the first instance, but the bridge
would connect to the impostor. Checking the pipe server's owner is left for
later; a single-user desktop is not exposed.

## Code layout

- `oa-mcp/src/lib.rs`: the `Session` gains the revision count, dirty and
  undo guards, the in-memory results copy, and the new tools' methods.
- `oa-mcp/src/server.rs`, new: the rmcp tool router, moved out of `main.rs`
  and written against a `Host` trait that runs a job on a session. The
  headless server's host is a mutex; the GUI's is a channel.
- `oa-mcp/src/socket.rs`, new: socket paths, the listener, and the attach
  bridge.
- `oa-mcp/src/main.rs`: with no arguments, the headless stdio server as
  today; with `--attach`, the bridge.
- `oa-gui/src/agent.rs`, new: the listener thread, the job channel, and the
  task that runs jobs on the document.
- `oa-gui/src/document.rs`: a `Session` in place of `Editor` and path.

## Verification

- Reference coverage and example parsing, as in section 1.
- `generate_combinations` against the model the GUI generator tests use.
- Modal: a cantilever with a tip mass has period 2π·√(m / (3EI/L³)).
- Spectrum: the same single degree of freedom under a flat spectrum has peak
  displacement Sa/ω² and base shear m·Sa.
- The undo guard and the unsaved-changes guard, driven through the session.
- End to end over the socket: an rmcp client connects through
  `oa-mcp --attach` to a listener serving a session, builds a model, and
  analyses it.
- By hand: an agent attached to the running GUI builds a frame, and the
  frame and its deformed shape appear in the view.

## Status

Implemented 2026-09-28 on branch `agent-interface-m7`. The tests are in
`crates/oa-mcp/tests/`: `reference.rs` covers section 1, `phase_m7.rs`
sections 2 and 3 and the guards, and `attach.rs` runs end to end through
the bridge binary.

Checked by hand on Linux (Hyprland) against the running GUI. An agent
script went through `oa-mcp --attach` to start a model, build a two-bay,
two-storey frame, generate its combinations, and analyse it. The frame
appeared as the commands landed, the deformed shape came up when the
analysis finished, and the status bar read "Agent attached". Ctrl+Z in the
window took back the agent's whole batch as one step, and the agent's
`undo` after it was refused, as was `new_model` over the unsaved frame.

Decided while building:

- When an agent's analysis finishes, the view turns the deformed shape on,
  as Ctrl+R does. When an agent replaces the model, the view zooms to fit,
  as opening a file does.
- The GUI prints why agents cannot attach to stderr as well as in the
  status bar's tooltip. The usual cause is an `OA_SOCKET` path over the
  roughly 100-byte limit on socket paths.
- The GUI does not remove its socket file on quit, since the socket thread
  is not joined. The next start finds that nothing answers on it and
  replaces it, and `--attach` reports "no open-analysis GUI is serving
  agents" with the connection error.
- A tool call that panics on the GUI thread is caught and reported to the
  agent, so it cannot take the window and the person's unsaved work with
  it.
- A named pipe cannot half-close. On Windows the bridge therefore exits as
  soon as the agent closes stdin, which closes the pipe and tells the GUI
  the agent has gone. On Unix it half-closes and waits for the GUI to
  finish answering.

Built and tested on Windows (`x86_64-pc-windows-gnu`) on 2026-09-29, where
`attach.rs` runs end to end over the named pipe. The GUI itself has not been
exercised by hand on Windows.
