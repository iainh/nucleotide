# Research: Scrolling Performance Bottlenecks

**Date**: 2026-06-16
**Question**: Where are likely performance bottlenecks in the editor scrolling code causing jank?
**Status**: Complete

## Context

The current editor scroll architecture is no longer the older parent-div wheel handler described in
`2026-06-12-editor-scrolling-architecture.md`. Wheel input is now owned by
`EditorSurface`, with a Nucleotide-owned `EditorViewport` and frame rendering pipeline in
`nucleotide-editor`. This note focuses on the current repaint path that runs during scroll.

## Findings

### 1. Soft-wrap visual row counting walks the whole document during paint

`render_native_editor_frame` first calls `EditorViewState::prepare_content_for_render`, which resolves
text metrics and calls `sync_content_layout_for_current_viewport`. That path reaches
`EditorDocumentMetrics::resolve`, where `visual_rows_for_text` calls Helix
`softwrapped_dimensions(text, text_format)` whenever soft wrap is enabled.

The same frame then calls `prepare_native_editor_frame`, which calls `sync_frame_layout`, then
`EditorViewport::sync_surface_layout`, then `editor_viewport_surface_metrics`, which resolves
`EditorDocumentMetrics` again. If gutter width changes, it can resolve a second time inside that same
surface metrics helper.

Evidence:

- `crates/nucleotide-editor/src/document_frame_painter.rs:415-444`
- `crates/nucleotide-editor/src/view_state.rs:172-183`
- `crates/nucleotide-editor/src/document_frame_painter.rs:311-328`
- `crates/nucleotide-editor/src/view_state.rs:185-217`
- `crates/nucleotide-editor/src/viewport.rs:539-561`
- `crates/nucleotide-editor/src/viewport.rs:668-695`
- `crates/nucleotide-editor/src/document_metrics.rs:17-35`
- `crates/nucleotide-editor/src/document_metrics.rs:62-65`

Impact: scrolling a large soft-wrapped file can do whole-document wrap measurement one or more times
per repaint. That is the highest-confidence source of frame-time spikes.

Recommended fix:

- Cache `EditorDocumentMetrics` by document revision, text format, viewport columns, gutter columns,
  and soft-wrap settings.
- Remove the duplicate content-metric priming in `render_native_editor_frame`, or make
  `prepare_native_editor_frame` reuse the already resolved metrics.
- Avoid requiring exact full-document visual row count on every scroll tick. Prefer cached counts,
  incremental invalidation, or a lazy/approximate scrollbar extent until the exact count is refreshed.

### 2. Viewport-to-Helix conversion repeats expensive visual-offset work every frame

`EditorViewport::sync_surface_layout_for_view` always calls `sync_from_helix_view` and then
`plan_view_position`, even when the GUI viewport is already the source of truth and there is no
external Helix scroll change. On a pending GUI scroll sync, it also calls `sync_view_position`, which
itself calls `plan_view_position`.

Those helpers use Helix visual-position conversion from the start of the document:

- `helix_viewport_snapshot` calls `visual_offset_from_block(text, 0, anchor, ...)`.
- `view_position_for_top_visual_row` calls `char_idx_at_visual_offset(text, 0, top_visual_row, ...)`.

Evidence:

- `crates/nucleotide-editor/src/viewport.rs:570-596`
- `crates/nucleotide-editor/src/viewport.rs:461-477`
- `crates/nucleotide-editor/src/viewport.rs:479-502`
- `crates/nucleotide-editor/src/viewport.rs:611-625`
- `crates/nucleotide-editor/src/viewport.rs:749-767`
- `crates/nucleotide-editor/src/viewport.rs:769-794`

Impact: scrolling deep into a soft-wrapped file can repeatedly scan from the document start to the
current viewport row/anchor. During wheel scroll, this can happen every frame, and possibly more than
once per frame.

Recommended fix:

- Skip `sync_from_helix_view` unless the document/view offset changed outside the native viewport path.
- Cache the last `top_visual_row -> ViewPosition` plan and invalidate only on document/layout/text-format
  changes or top-row changes.
- When the top row changes by a small delta, advance from the previous anchor/checkpoint instead of
  recomputing from row 0.

### 3. Syntax highlighting is rebuilt separately for every visible line

Frame construction maps each visible line to `unwrapped_highlighted_line` or
`soft_wrap_highlighted_line_runs`. Both call `highlight_line`. `highlight_line` creates a new syntax
highlighter and overlay highlighter for that line, then advances to the requested line range.

`syntax_highlight_window` also sets the highlight height to every line from the viewport anchor to EOF,
not just the visible range.

Evidence:

- `crates/nucleotide-editor/src/document_frame.rs:158-188`
- `crates/nucleotide-editor/src/highlight.rs:186-223`
- `crates/nucleotide-editor/src/highlight.rs:225-249`
- `crates/nucleotide-editor/src/highlight.rs:145-184`
- `crates/nucleotide-editor/src/highlight.rs:283-289`
- `crates/nucleotide-editor/src/highlight.rs:291-312`

Impact: a repaint for 40 visible rows can instantiate and advance 40 independent highlighters. This
turns viewport highlighting into repeated work and can be especially bad near the bottom of large files.

Recommended fix:

- Build one frame-level syntax highlighter and overlay highlighter, then stream through all visible
  line ranges in order.
- Limit the syntax highlighter range to visible rows plus a small guard band unless Helix requires a
  larger range for correctness.
- Cache highlighted line runs by document revision, visible line range, theme, mode, selection version,
  diagnostics version, and syntax layer version.

### 4. Text shaping cache hits still allocate line strings

The shaped-line cache keeps shaped lines across frames, which is good. But every cache lookup builds a
key containing an owned `String`, and line preparation also allocates strings:

- Unwrapped lines call `shared_line_text_without_trailing_newline`, which converts the rope slice to a
  `String`.
- `shape_line_cached` builds a `ShapedLineKey` by calling `line_text.to_string()`.
- Soft-wrap paint clones the visual-line `String` into a `SharedString` before lookup.

Evidence:

- `crates/nucleotide-editor/src/line_text.rs:7-17`
- `crates/nucleotide-editor/src/highlight.rs:225-249`
- `crates/nucleotide-editor/src/line_cache.rs:70-104`
- `crates/nucleotide-editor/src/line_cache.rs:242-258`
- `crates/nucleotide-editor/src/line_painter.rs:96-123`
- `crates/nucleotide-editor/src/line_painter.rs:154-172`
- `crates/nucleotide-editor/src/soft_wrap.rs:342-417`

Impact: this is lower priority than whole-document wrap measurement and repeated highlighter setup, but
it is still repeated on scroll repaint and can increase allocator pressure.

Recommended fix:

- Avoid full line text in the shape-cache key when possible. Use document revision plus line/segment
  range, viewport width, font size, and run hash.
- If text must remain in the key, store `SharedString` or an interned string in the key to avoid
  `to_string()` on cache-hit lookup.
- Reuse soft-wrap visual-line buffers or store shaped-line-ready `SharedString` in the render plan.

### 5. Debug logging in paint paths can amplify jank when enabled

There are multiple `debug!` calls in frame planning, line painting, cursor painting, highlighting, and
viewport sync. With debug logging enabled, scroll repaint can emit many events per frame.

Evidence:

- `crates/nucleotide-editor/src/document_frame_painter.rs:424-429`
- `crates/nucleotide-editor/src/document_frame_painter.rs:499-547`
- `crates/nucleotide-editor/src/document_frame_painter.rs:740-777`
- `crates/nucleotide-editor/src/document_frame_painter.rs:875-947`
- `crates/nucleotide-editor/src/highlight.rs:297-299`
- `crates/nucleotide-editor/src/highlight.rs:515-524`
- `crates/nucleotide-editor/src/viewport.rs:491-499`
- `crates/nucleotide-editor/src/scroll_manager.rs:200-207`

Impact: probably not the default jank source if `RUST_LOG=info`, but easy to trigger during debugging,
which is exactly when scroll investigations happen.

Recommended fix:

- Move per-line/per-highlight logs to `trace!`, gate them behind a dedicated diagnostic feature, or
  sample/aggregate them.
- Keep one concise frame timing log around the whole repaint path.

## Probably Not The Primary Bottleneck

### Wheel event handling itself is short

The `EditorSurface` wheel handler checks bounds, converts the delta, updates the viewport, calls the
optional scroll callback, notifies the document entity, and stops propagation. This path is not obviously
heavy by itself.

Evidence:

- `crates/nucleotide-editor/src/surface.rs:234-256`

### Per-frame line layout cache clearing is less concerning than it looks

`EditorViewState::sync_frame_layout` clears the per-frame layout store, but `LineLayoutCache::clear`
does not clear shaped lines. It primarily resets hit-test layout records that are repopulated by paint.

Evidence:

- `crates/nucleotide-editor/src/view_state.rs:206-208`
- `crates/nucleotide-editor/src/line_cache.rs:128-134`

### Existing benchmark coverage does not hit the expensive path

The only `nucleotide-editor` benchmark currently covers `LineLayoutCache` lookup. That is useful, but it
does not measure soft-wrap metrics, viewport sync conversion, highlighting, shaping, or full frame prep.

Evidence:

- `crates/nucleotide-editor/benches/line_layout_cache.rs:22-84`

## Options Considered

| Option | Pros | Cons | Effort |
|--------|------|------|--------|
| Cache and reuse soft-wrap/document metrics | Targets the likely largest full-document cost | Needs careful invalidation across document edits, wrap settings, and viewport width | Medium |
| Stream highlighting once per frame | Removes repeated highlighter setup and repeated scanning | Requires refactoring highlight APIs around frame-level state | Medium |
| Cache viewport visual-row conversions | Reduces deep-file soft-wrap scroll cost | Needs source-of-truth clarity between Helix offsets and native viewport | Medium |
| Reduce allocation in text shaping keys | Lowers allocator pressure during repaint | Smaller win than metric/highlight fixes | Small to medium |
| Add instrumentation only | Gives proof before refactor | Does not directly fix jank | Small |

## Recommendation

Start with instrumentation, then fix the two largest likely costs.

1. Add timing spans around:
   - `render_native_editor_frame`
   - `prepare_content_for_render`
   - `prepare_native_editor_frame`
   - `EditorDocumentMetrics::resolve`
   - `EditorViewport::sync_surface_layout_for_view`
   - `editor_document_frame`
   - total highlight-run construction
   - total line shaping/paint
2. Add Criterion benches for generated large documents:
   - non-wrap frame prep
   - soft-wrap frame prep
   - deep-scroll soft-wrap viewport sync
   - highlighted visible-range construction
3. Remove duplicate `EditorDocumentMetrics::resolve` work and cache soft-wrap visual row counts.
4. Refactor highlighting to stream once per frame instead of rebuilding per visible line.
5. Cache or incrementally update viewport visual-row-to-Helix-position conversions.

## Open Questions

- Do users see the worst jank mostly with soft wrap enabled, or also in unwrapped files?
- Does jank correlate with large files, long lines, syntax-highlighted files, many diagnostics, or deep
  scroll positions?
- Is `RUST_LOG` normally set to `debug` during the reported jank?
- Do we require exact scrollbar thumb size immediately for all soft-wrapped files, or can it be refined
  after asynchronous/lazy measurement?

## References

- `crates/nucleotide-editor/src/surface.rs`
- `crates/nucleotide-editor/src/viewport.rs`
- `crates/nucleotide-editor/src/view_state.rs`
- `crates/nucleotide-editor/src/document_metrics.rs`
- `crates/nucleotide-editor/src/document_frame.rs`
- `crates/nucleotide-editor/src/document_frame_painter.rs`
- `crates/nucleotide-editor/src/highlight.rs`
- `crates/nucleotide-editor/src/line_cache.rs`
- `crates/nucleotide-editor/src/line_painter.rs`
- `crates/nucleotide-editor/src/soft_wrap.rs`
- `crates/nucleotide-editor/benches/line_layout_cache.rs`
