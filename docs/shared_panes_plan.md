# Shared-pane migration plan

## Status and scope

This is the implementation contract, not a completion report. The starting
revision is `b23aaced1fcf2eb9ca220d7364d135e051c9803e`, source local `main`,
one unpushed commit ahead of `origin/main` when work began. The source integration
thread supplied a Linux baseline of 2,257 passed, 0 failed and 11 ignored tests
across 50 test/doc-test suites, excluding performance tests. That result is
source-reported; it is not a rerun by the documentation owner.

The migration covers same-window document, image and terminal panes, including
the bottom dock. Cross-window dragging, session persistence across restart and
simultaneous rendering of one PTY in two viewports are out of scope. Do not
redesign the event system, emulator, remote protocol or generic UI framework.

See [verification and evidence](shared_panes_testing.md) for the executable
baseline, acceptance matrix and native platform scenario. API names below are
responsibilities unless explicitly identified as existing interfaces; packages
publish their concrete interfaces before consumers write integration tests.

## Sources of truth

### Workspace topology and content placement

A plain-Rust, app-local pane model owns the content-neutral tree. Generational
`PaneId` is independent of Helix `ViewId`. Ordered, weighted split containers
own axes, children and stable split identities; pane state owns tab order,
active tab, activation history and dock visibility. GPUI entities and focus or
scroll handles belong in presentation state keyed by pane, not in the model.

Content resources keep their existing owners: Helix owns documents and editing
history, the image subsystem owns decoded resources/cache data, and the terminal
runtime owns processes, IO and emulator models. A content key is not a placement:
documents and images can occur in several panes, uniquely within each pane.
A terminal session has exactly one placement, including hidden dock placement.

Distinguish these operations: tab placement close, pane close, buffer close
everywhere, terminal termination, dock hide and application quit. Do not infer
one operation from another's incidental editor effects.

### Editing slots are not GUI tiles

A pane with real document tabs can retain a Helix editing slot while an image
or terminal is active. A content-only pane has no backing document or editor
view. Keep the existing per-pane switching/history behaviour rather than
allocating an editor view for every background tab. Explicit mappings connect
panes, retained slots and visible document presentations.

`Editor::switch(Action::Replace)` updates history, modes and focus hooks, and can
delete an undisplayed empty scratch. Preserve scratch documents still referenced
by workspace tabs. Buffer close can replace documents, remove views or create
fallbacks; none of these effects alone is a workspace pane-close request.

### One-way geometry

Workspace allocation in logical pixels is authoritative. Subtract each pane's
chrome, then project the content rectangle into an editor viewport using editor
metrics, terminal rows/columns using terminal metrics, or an image viewport.
Store weights, not Helix cell rectangles. Generate resize handles from split
nodes, not inferred shared edges. Document compositor-to-window coordinate
conversion for cursor, IME and overlays.

Hosted editor projection must not cause Helix reflow to become GUI authority.
Do not remove the existing clone-based viewport safeguard until its replacement
has a single explicit geometry writer. Render must not mutate topology.

## Hosted bridge and command contract

Add a narrow hosted mode to the vendored editor/tree. Default standalone Helix
semantics remain unchanged. The host needs slot creation, retention and release;
host-supplied viewports; typed ordered document/slot/split effects; and a
projection path that suppresses echoed requests. Drain effects at command, job
and maintenance boundaries, not by comparing leaves after each render.

Route workspace operations through typed ordered commands: navigation,
next/previous, rotation, swaps, transpose, split current/new, close, keep-only,
quit and open into a requested split. Preserve Helix key parsing, pending
prefixes, counts, registers, macros and typable aliases/arguments. Do not add a
second string-key interpreter in Workspace.

A mapped split → focus → edit sequence must apply each host operation and
project the resulting focus before resuming editing. Stop/reject editing if the
destination is content-only; never edit the last hidden document. Cancel the
continuation after a failed host operation, including suspended macro input.
`:wq` validates and flushes writes before yielding close; failed writes and
cancelled validation must not close anything. Proposed dispatch/resume methods
are not a substitute for testing the delivered public behaviour.

### Structural mutation inventory

Every row needs an explicit adapter or a test before authority cutover:

| Producer | Required contract |
| --- | --- |
| Native keymaps, command lists, macros and static commands | Ordered typed operations and continuation |
| Native command prompt, aliases, numeric goto and static fallback | Preserve parsing, errors, arguments, counts and force semantics |
| `Editor::switch` Replace/Load/splits/`SplitAt` | Preserve originating destination and document membership |
| `new_file`, file open and remote open | Explicit destination and activation |
| Buffer close/all/others and preview cleanup | Membership removal distinct from editor fallback |
| CLI files and split flags | One-time topology import preserving initial shape |
| Picker, navigation, LSP and delayed callbacks | Carry source/destination; reject stale targets |
| Workspace-edit resource operations | Preserve split versus replace intent |
| GPUI document/gutter focus | Map editor slots to panes, with one focus owner |
| Jobs, compositor callbacks and maintenance | Ordered effects before presentation |
| Directory/project replacement and shutdown | Explicit lifecycle transaction and generation checks |

Replace both tree-empty quit assumptions: command execution's `should_close()`
check and application redraw/maintenance's zero-view `ShouldQuit` inference.
Terminal-only and image-only workspaces must continue maintenance. Topology
import is allowed for startup and shadowing, never perpetual reverse-sync after
cutover.

## Product policies

### Tabs, splits and close

- Activate on click release. Starting an inactive-tab drag does not activate it.
- Moves preserve identity and presentation state. Target document/image copies
  deduplicate; image moves transfer zoom/scroll, not recreate it.
- Document/image splits duplicate presentation; terminal splits create a new
  session. Duplicate images have independent zoom and scroll.
- Matching-axis sibling splits redistribute siblings equally. Target-only edge
  splits divide only the target allocation, preserving unrelated weights.
- A final-tab cross-pane move and source collapse are atomic. A sole-tab
  self-edge drop is a no-op, preserving pane identity and history in main,
  nested and dock layouts.
- Transpose changes the focused pane's immediate parent axis, not descendant
  axes recursively. Divider resize uses the same propagated subtree minima as
  rendering, not merely leaf minima.
- Close preserves the configured successor, including `LeftNeighbour`, rather
  than replacing that policy with adjacent/MRU defaults. Empty nonroot panes
  collapse. Ordinary final-tab close leaves an empty workspace pane; explicit
  final-pane quit preserves application-exit behaviour with unsaved validation.
- Closing one shared document copy keeps the buffer. Closing its final copy or
  closing the buffer everywhere validates unsaved data before mutation.
- Preserve no-argument GUI `bc`/`bclose`/`buffer-close`/`close` placement-close
  interception. Explicit buffer-targeted Helix operations validate and remove
  every affected placement; they do not imply pane-close or application quit.
- Pane close/keep-only transfers otherwise-orphaned resources to a survivor,
  including hidden terminals. Explicit terminal-tab close terminates only that
  session, with running-session/task confirmation unless forced.
- Image resource lifetime follows layout membership, including hidden/unmounted
  placements, not merely entries in a lazy presentation map.

### Terminal lifecycle and unified dock

Keep terminal state by stable session identity: spawn specification, lifecycle,
focus/entity, last nonzero bounds, optional run/activity and workspace/backend
generation. Prepare/reserve identity before asynchronous environment resolution.
Cancellation must prevent late registration or resurrection; preparation cannot
revive a tombstone. Queue initial input until registration.

Termination requests and observed spawned/failed/exited facts are distinct.
Deliver identified worker facts through the existing typed application flow,
never worker-thread GPUI mutation or a competing polling consumer. Finalize
completion once by session/run, even while inactive. Unknown exit status means
completed with unknown status, not pending and not invented success.

Opening another session, moving or hiding never kills a session. Hidden sessions
keep their last nonzero size. Only the visible owning surface resizes its own
session; unmounts never send zero-size or competing dimensions. Keep exited
output and spawn-failure details until explicit close. Keep IO off the UI loop.
Preserve service-owned remote PTY, proxy fallback, cwd/path mapping, resolved
environment and session-specific initial input.

The bottom dock uses the same pane model and renderer, with remembered placement,
size and hide/reveal state, not a second singleton terminal architecture:

- Toggle hides/reveals the dock, creating one dock session if none remains.
- Reveal explicitly restores the remembered dock pane and tab; visibility alone
  does not establish the keyboard-focus destination.
- New terminal always creates a dock session; new centre terminal creates one
  in the active central pane.
- Open in terminal creates a session at the requested cwd, dock by default.
- Runnable output reveals its dock tab without replacing another session.
- Dock/centre drag transfers placement, never mirrors a PTY. An emptied dock
  toggle creates a new session rather than stealing a central terminal.

### Input and focus

Distinguish active pane/tab, last editor context and actual keyboard focus.
Route input by focused content. A coordinator's last-terminal handle is not the
identity of every session. Restore overlay focus to the initiating placement,
or a surviving active pane if removed. Clear document completion/hover/IME state
on non-document activation. Preserve terminal control keys, especially shell
Ctrl-W; explicit application shortcuts navigate from terminals. Editor Helix
pane navigation may enter image/terminal panes without leaking subsequent edits.

## Work packages and ownership

| Package | Owned scope | Prerequisites |
| --- | --- | --- |
| A — Model | `workspace/pane_layout.rs`, model tests | Frozen identity/operation policies |
| B — Hosted bridge | Vendored editor/tree, `pane_bridge.rs`, bridge tests | Hosted contract and early seam spike |
| C — Commands | `editor_input.rs`, `helix_command.rs`, `workspace_commands.rs` | A operations and B ordered effects |
| D — Presentation | Pane render/interactions, split resize, view manager | A and B viewport boundary |
| E — Terminals | Runtime, terminal controller/events, terminal view/panel | Identified lifecycle; D not needed initially |
| F — Images | Image resources/presentations and their tests | A/D placement boundary |
| G — Serial integration | Shared Workspace/Application/types, workspace edits, document/overlay/input wiring, typed-tab migration and legacy deletion | Integrate A–F serially |
| H — Verification | Dedicated contract/scenario modules, CI, these two docs | Baseline now; mixed scenarios after G |

G alone edits large shared integration files and registers modules/exports.
Existing shared test files also have one owner; other packages add dedicated
modules. E owns terminal event changes; consumers do not edit that contract.
The refined delegation reserves `tab.rs` and typed-tab migration to G. Platform
checks use one exact integration revision, not independently moving branches.

## Staged implementation gates

Each stage leaves the application runnable. Stage 0 precedes Stages 1–8.

| Stage | Work and decisive exit gate |
| --- | --- |
| 0 — Characterize | Freeze click/drag/close, scratch, preview, selection/scroll, aliases, mapped commands, cancellation, toggle, remote launch and task retention. New characterizations pass on the exact base; inventory structural producers. |
| 1 — Shadow model | Implement identities, weighted topology, membership, geometry, navigation and invariants. Import existing layouts without changing rendering; model and baseline document layouts agree. |
| 2 — Hosted seam | Keep current GUI. Prove two real document slots plus a content-only pane through split → focus → replace → buffer close → final editor close. No phantom pane, stale edit, scratch loss, geometry reflow or unintended quit; standalone tests unchanged. Stop if ordering cannot be preserved. |
| 3 — Commands/boundaries | Typed commands and continuations, aliases and lifecycle validation; adapt all producer paths and quit assumptions. Every inventory row has an adapter/test; Helix topology is not final GUI authority. |
| 4 — Document GUI | Switch document tiling, resize, navigation and drag to workspace identities before images/terminals. Preserve nested/unequal splits, CLI shape and document behaviour; render is read-only. |
| 5 — Images | Remove active-image layout authority and backing-document requirements. Image-only startup/splits, independent duplicated viewports, local/remote cache and last-document close all work. |
| 6 — Terminals | First two independent dock sessions, then central placement. Prove distinct processes/IO/actual sizes, move identity, hidden output, inactive completion and close-during-spawn cancellation. |
| 7 — Unified dock/focus | Remove reverse-sync, singleton terminal resize/exit polling and global active-image authority. No GUI tiles derived from editor-tree views; remaining iteration has an editing/maintenance purpose. |
| 8 — Hardening | Linux full-workspace CI, retained platform checks, migration docs and native same-scenario evidence on each platform, clearly separated from compilation/headless results. |

Prefer the narrow hosted seam over fake backing documents, making all of Helix
content-neutral or replacing view storage at once. Those alternatives either
retain the unwanted authority coupling or broaden the fork and regression risk.
The cheapest early risk checks are ordered continuations, hidden-document input,
scratch/history retention, fractional geometry/DPI, late spawn cancellation and
both quit assumptions. Do not merge GUI authority cutover before B's seam passes.
