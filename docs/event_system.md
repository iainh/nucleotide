# Application Event System

Nucleotide has one application-level GPUI event model: `Update` in
`crates/nucleotide/src/types.rs`. `Application` emits it, while `Workspace` and
`OverlayView` subscribe to the variants they own.

## Event flow

```diagram
┌────────────┐   typed input   ┌───────────────────┐
│ GPUI input │───────────────▶│ EditorInputBridge │
└────────────┘                 └─────────┬─────────┘
                                       │
                         typed request │ Helix command
                                       ▼
┌─────────────┐  HelixEvent   ┌───────────────────────┐
│ Helix hooks │──────────────▶│ Application           │
└─────────────┘   (private)    │ handle_helix_event()  │
                              └───────────┬───────────┘
                                          │ Update
                                          ▼
                              ┌───────────────────────┐
                              │ Workspace / Overlay   │
                              └───────────────────────┘
```

### GPUI to Helix

Keyboard input is translated by `EditorInputBridge`. Native UI commands, such
as opening a picker, become typed requests and emit an `Update` directly.
Commands owned by Helix execute through Helix's command system.

### Helix to GPUI

Helix hooks in `nucleotide-core/src/event_bridge.rs` send the private
`HelixEvent` transport over an MPSC channel. `Application` drains and coalesces
that channel, enriches each event from editor state, performs required state
synchronization, and handles it once in `handle_helix_event`.

The handler emits a direct `Update` domain variant:

- `Update::Document(DocumentEvent)`
- `Update::Lsp(LspEvent)`
- `Update::Ui(UiEvent)`
- `Update::Workspace(WorkspaceEvent)`

There is no second application event wrapper. `HelixEvent` is an integration
transport only and is not exposed to UI subscribers.

## Application events

`Update` also owns GPUI-local payloads that cannot live in a data-only shared
crate, including picker callbacks and GPUI entities. Control requests such as
`ShowFilePicker`, `OpenFile`, and `Redraw` use the same enum.

Component implementation events remain local when they are not application
concerns. Examples include `DismissEvent`, completion acceptance events, and
file-tree interaction events. Their owning component translates them to an
`Update` only when another application component must react.

## Adding an event

1. If it is an application fact or request, add one `Update` variant and handle
   it in the owning subscriber.
2. If it originates in Helix, add the smallest transport variant to
   `HelixEvent` and translate it to `Update` in `Application::handle_helix_event`.
3. If it is private to one component, keep it as a component-local GPUI event.
4. Do not add wrapper enums, string-based command mappings, or another channel
   for the same event.

Handlers run on the application loop. Keep them non-blocking; move expensive
work to background tasks and emit the result back through `Update`.

## Bridge lifetime

The Helix event sender is stored in a process-global `OnceLock`. This matches
the current single-application integration. Tests and future multi-window work
must treat it as a singleton; use an explicit bridge handle before attempting
to support multiple independent editor integrations in one process.
