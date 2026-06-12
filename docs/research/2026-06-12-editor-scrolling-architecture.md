# Editor Scrolling Architecture Investigation

Date: 2026-06-12
Status: Complete

## Question

The editor still feels like it is not scrolling correctly after local scroll manager changes. Investigate whether the root cause is our synchronization with Helix, and what we could improve by reimplementing more of the editor viewport instead of relying on `helix-term` scrolling code.

## Short Answer

Yes, the current design is still too coupled to Helix's terminal scroll model. The biggest issue is not just the scroll math. The editor currently has several independent scroll authorities:

- Helix `doc.view_offset(view_id)`
- Nucleotide `ScrollManager`
- scrollbar state
- transient GPUI wheel-event state
- Helix terminal command behavior through `helix_term::commands::scroll`

Those are mirrored in multiple directions during input and paint. That makes sub-line scrolling, soft-wrap scrolling, and cursor scrolloff behavior fragile. The most promising direction is to make a Nucleotide-owned `EditorViewport` the source of truth for GUI scrolling, then sync into Helix view offsets only at explicit integration boundaries.

## Current Flow

The wheel handler in `crates/nucleotide/src/document.rs` is attached to the parent `div` around `DocumentElement`:

- `event.delta.pixel_delta(line_height)` converts GPUI wheel deltas to pixels.
- `ScrollManager::scroll_by_delta(delta)` updates local pixel offset.
- If whole lines were crossed, it calls `helix_term::commands::scroll`.
- If only fractional pixels changed, it only calls `window.refresh()`.

Paint then syncs back and forth:

- If `scrollbar_changed`, it writes to Helix via `doc_mut.set_view_offset`.
- It reads Helix `doc.view_offset(view_id)`.
- It calls `set_scroll_position_from_helix_preserving_intra_line_offset`.
- It renders non-wrapped content from `ScrollManager::visible_line_range`.
- It renders wrapped content from Helix `view_offset` and `DocumentFormatter`.

This means scroll state is not owned in one place.

## Key Findings

### 1. Sub-Line Scroll May Not Invalidate the Editor Entity

For fractional wheel deltas, the current handler mutates `ScrollManager` and calls `window.refresh()`, but it does not notify the `DocumentView` entity.

GPUI's own scroll utilities usually call `cx.notify(view_id)` after mutating scroll offset. For example, `gpui-component`'s `ScrollableMask` updates the `ScrollHandle` and then notifies the owning view.

This is a high-priority failure hypothesis: the scroll manager can change while the element tree for `DocumentView` is not invalidated through the usual entity path.

Recommended experiment:

- Move wheel handling into an explicit GPUI-style scroll mask or hitbox over the editor paint area.
- After local offset changes, call `cx.notify(window.current_view())`.
- Keep `cx.stop_propagation()` when the editor handles the event.
- Add short-lived debug logs for wheel delta, old/new local offset, Helix view offset, and visible line range.

### 2. Event Handler Placement Is Brittle

The scroll handler is on the wrapping `div`, while `DocumentElement` owns its own interactivity and mouse hitboxes. GPUI's `on_scroll_wheel` dispatch checks whether a hitbox should handle scroll. That makes parent-level wheel handling around a custom element a less direct match than the GPUI component pattern.

Recommended improvement:

- Use an explicit scroll capture layer for the editor viewport.
- Make that layer responsible for wheel input, propagation, and invalidation.

This is separate from scroll math and may explain why previous math changes did not affect perceived behavior much.

### 3. `helix_term::commands::scroll` Is the Wrong Abstraction for GUI Wheel Scroll

`helix_term::commands::scroll` is a terminal command. It updates Helix `view_offset`, and even with `sync_cursor = false`, it still contains cursor/scrolloff-related behavior when the cursor leaves the viewport.

That is appropriate for a terminal renderer with row-based cells. It is not a natural fit for a GPUI editor that wants pixel deltas, smooth partial-line movement, a GUI scrollbar, and independent painting.

Recommended improvement:

- Stop calling `helix_term::commands::scroll` from GUI wheel input.
- Keep using lower-level Helix primitives where useful:
  - `doc.set_view_offset`
  - `DocumentFormatter`
  - `char_idx_at_visual_offset`
  - `screen_coords_at_pos`
- Implement GUI scroll policy in Nucleotide.

### 4. Soft-Wrap Height Is Computed Then Discarded

`DocumentElement::paint` computes a `_visual_total_lines` value for soft wrap, but then resets `ScrollManager::total_lines` from `doc.text().len_lines()`.

That means scrollbar range and content height are raw document lines, not visual rows. For wrapped content, the scroll model cannot match what the renderer displays.

Recommended improvement:

- Track content height in visual rows, not raw document lines, when soft wrap is active.
- Store this in the new viewport model as `content_visual_rows`.

### 5. Rendering Has Two Scroll Models

Non-wrap rendering mostly uses `ScrollManager`:

- local line range
- local fractional offset
- gutter shift from fractional offset

Soft-wrap rendering still starts from Helix `view_offset` and `DocumentFormatter`, then overlays the local fractional offset.

Recommended improvement:

- Use one viewport abstraction for both wrap and non-wrap paths.
- Convert pixel offset into a visual-row anchor and fractional row offset.
- Let the renderer consume that anchor consistently.

### 6. Current Tests Cover Math, Not Behavior

The existing tests verify `ScrollManager` pixel/line crossing behavior, but they do not prove:

- wheel events reach the editor
- fractional scroll changes invalidate the view
- Helix view offset is not overwriting local scroll
- scrollbar range matches visual wrapped rows
- paint output changes after small deltas

Recommended improvement:

- Add unit tests around the new viewport state machine.
- Add a narrow integration or diagnostic path for scroll input -> viewport offset -> rendered visible range.
- Keep short-lived trace logging behind a feature or debug env var until the behavior is pinned down.

## Reimplementation Options

### Option A: Minimal Diagnostic Fix

Keep the current architecture, but replace the parent wheel handler with an explicit scroll mask/hitbox and notify the editor entity on handled scroll.

Pros:

- Smallest change.
- Directly tests the event delivery and invalidation hypothesis.
- Can preserve current scroll manager tests.

Cons:

- Does not fix the deeper multiple-owner design.
- Still relies on `helix_term::commands::scroll` for whole-line wheel movement.

This is the best immediate next step before a larger refactor.

### Option B: Nucleotide-Owned `EditorViewport`

Introduce a GUI viewport model as the source of truth:

```rust
struct EditorViewport {
    pixel_offset_y: Pixels,
    top_visual_row: usize,
    fractional_y: Pixels,
    anchor: usize,
    vertical_offset: usize,
    horizontal_offset_cols: usize,
    viewport_height: Pixels,
    line_height: Pixels,
    content_visual_rows: usize,
}
```

Wheel input, scrollbar movement, cursor reveal, and rendering all read/write this model. Helix `view_offset` is updated only when needed as an integration detail.

Pros:

- Removes the main source of scroll fights.
- Supports smooth pixel scrolling naturally.
- Lets us implement GUI-specific cursor reveal and scrolloff behavior.
- Gives tests a clean target.

Cons:

- Requires careful conversion between pixel offsets, visual rows, anchors, and Helix positions.
- Needs focused work around soft wrap and document edits.

This is the recommended medium-term architecture.

### Option C: GPUI-Native Scroll Container

Use GPUI `ScrollHandle` and a scrollable container pattern similar to `gpui-component`.

Pros:

- Aligns with GPUI event and invalidation behavior.
- Gives scrollbars and masks a known pattern.

Cons:

- The editor is custom-painted, not a normal list of laid-out child elements.
- GPUI needs content sizing information, which we would still need to compute from visual rows.

This is useful for event handling and scrollbar integration, but probably not sufficient by itself.

### Option D: Fully Virtualized Visual-Row Renderer

Render the editor as a virtualized list of visual rows.

Pros:

- Natural fit for GUI scrolling and large files.
- Can make scrolling and scrollbar range very explicit.

Cons:

- Large rewrite.
- Harder to preserve selections, cursors, diagnostics, virtual text, gutters, syntax highlighting, and overlays.

This may be a long-term direction, but it is too large for the immediate scrolling fix.

## Recommended Path

1. Implement the minimal diagnostic fix:
   - explicit editor scroll mask or hitbox
   - local offset mutation
   - `cx.notify(window.current_view())`
   - short-lived debug logs

2. Remove `helix_term::commands::scroll` from GUI wheel input:
   - convert wheel deltas into `EditorViewport` changes
   - sync Helix `view_offset` from the viewport only after local state is updated

3. Build `EditorViewport`:
   - own vertical pixel offset
   - derive visual row anchor
   - preserve fractional row offset
   - compute content height from visual rows
   - support both wrap and non-wrap rendering

4. Move cursor reveal/scrolloff policy into Nucleotide:
   - keyboard and editing paths should call a GUI-aware `ensure_cursor_visible`
   - avoid mutating viewport during paint

5. Add tests:
   - pixel deltas accumulate without crossing a line
   - whole-line deltas update visual row anchor
   - Helix view offset sync does not discard fractional offset
   - soft-wrap content height uses visual rows
   - cursor reveal does not fight manual scroll

## Open Questions

- Does the current parent `on_scroll_wheel` fire for every precise trackpad delta, or only when the parent hitbox wins the scroll hit test?
- Does `window.refresh()` repaint custom elements reliably without `cx.notify`, or only mark the window dirty after an existing invalidation?
- How much smooth sub-line scrolling do we want for soft-wrap rows with variable visual content?
- Should manual wheel scroll temporarily suppress cursor scrolloff until the next cursor-moving command?
- Should scrollbar dragging operate in pixels, visual rows, or raw document lines?

## Source References

- `crates/nucleotide/src/document.rs`
  - wheel handling around the editor paint area
  - paint-time Helix/view sync
  - split wrap and non-wrap rendering paths
  - soft-wrap visual line count computation
- `crates/nucleotide-editor/src/scroll_manager.rs`
  - current local pixel scroll state
  - fractional line offset preservation
- `helix-term/src/commands.rs`
  - terminal scroll command behavior
- `helix-view/src/view.rs`
  - cursor reveal and screen coordinate helpers
- `helix-term/src/ui/editor.rs`
  - terminal rendering starts from Helix `view_offset`
- `crates/nucleotide/src/file_tree/view.rs`
  - example of GPUI `ScrollHandle` and tracked scroll usage
- `gpui-component/src/scroll/scrollable_mask.rs`
  - scroll mask pattern: mutate offset, notify view, stop propagation
