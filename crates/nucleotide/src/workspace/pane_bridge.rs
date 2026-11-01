//! Boundary between workspace-owned panes and hosted Helix document slots.
//!
//! Register this module in `workspace` when installing the hosted cutover. The
//! workspace owns PaneId, tab membership, active content and logical-pixel bounds;
//! a terminal/image-only pane has no editor slot. A pane may retain a hidden real
//! document slot while another kind of content is active.
//!
//! Bootstrap existing slots once when enabling `Editor::enable_hosted`. Drain
//! `Editor::drain_hosted_effects` in order after each command/job and before its
//! continuation, not during rendering. Creation carries the exact Helix Action
//! and source slot: regular splits redistribute siblings; SplitAt divides only
//! its source. DocumentClose removals are buffer fallback, NOT pane-close intent.
//! DocumentReplaced carries both the old and surviving document plus its reason.
//!
//! Explicit pane split/focus/navigation/close belongs to the host command router.
//! Apply it to workspace state, then use the silent `project_hosted_*` Editor
//! methods. Project focus with None for content-only focus; do not resume editing
//! against the hidden `tree.focus`. Slot creation/replacement focuses its target,
//! so project the final workspace focus before resuming a command continuation.
//!
//! Project cell viewport sizes derived from the authoritative content bounds via
//! `Editor::project_hosted_viewport`. The existing nucleotide_editor viewport
//! sizing helpers account for gutters/status rows; retain its cloned-view paint
//! safeguard. Never reconstruct GUI pane geometry from Helix's slot tree.
//!
//! Zero hosted slots is valid: `Editor::should_close` no longer implies quit.
//! The workspace decides quit/maintenance based on its content, not slot count.
//! All hosted real documents, including empty scratch tabs, remain until an
//! explicit buffer close. Releasing a slot only releases its editing state.

pub(super) use helix_view::editor::hosted::{
    DocumentReplacementReason, HostedEffect, HostedError, SlotRemovalReason,
};
