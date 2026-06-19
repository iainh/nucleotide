use std::collections::BTreeMap;

use gpui::{
    App, Bounds, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString, Styled,
    TextStyle, Window, div, px,
};
// Import helix's syntax highlighting system
use helix_view::ViewId;
use nucleotide_events::v2::run::ResolvedTask;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::theme_manager::HelixThemedContext;

use crate::{Core, Input, InputEvent};
use nucleotide_editor::{
    EDITOR_MINIMUM_VIEWPORT_COLUMNS, EditorCursorReveal, EditorLayout, EditorPointerSelectionPhase,
    EditorSurfacePointerEvent, EditorViewLayoutSnapshot, EditorViewState, NativeEditorFramePalette,
    NativeEditorFrameRenderParams, NativeEditorFrameThemeStyles, NativeEditorView,
    ViewportScrollUpdate, log_pointer_selection_outcome, render_native_editor_frame,
    run_gutter_extra_columns,
};

fn handle_editor_pointer_selection(
    core: &Entity<Core>,
    view_id: ViewId,
    editor_state: &EditorViewState,
    phase: EditorPointerSelectionPhase,
    event: EditorSurfacePointerEvent,
    cx: &mut App,
) {
    let outcome = core.update(cx, |core, cx| {
        let outcome = editor_state.handle_pointer_selection_for_view_outcome(
            &mut core.editor,
            view_id,
            phase,
            event,
        )?;

        if outcome.changed() {
            cx.notify();
        }

        Some(outcome)
    });

    if let Some(outcome) = outcome {
        log_pointer_selection_outcome(outcome);
    }
}

fn focus_editor_view(core: &Entity<Core>, view_id: ViewId, cx: &mut App) {
    core.update(cx, |core, cx| {
        if core.editor.tree.try_get(view_id).is_none() {
            return;
        }

        if core.editor.tree.focus != view_id {
            core.editor.focus(view_id);
        }

        cx.emit(crate::Update::ViewFocused { view_id });
        cx.notify();
    });
}

fn run_gutter_task(core: &Entity<Core>, view_id: ViewId, task: ResolvedTask, cx: &mut App) {
    core.update(cx, |core, cx| {
        if core.editor.tree.try_get(view_id).is_none() {
            return;
        }

        if core.editor.tree.focus != view_id {
            core.editor.focus(view_id);
        }

        cx.emit(crate::Update::ViewFocused { view_id });
        cx.emit(crate::Update::RunTask(task));
        cx.notify();
    });
}

pub struct DocumentView {
    core: Entity<Core>,
    input: Option<Entity<Input>>,
    view_id: ViewId,
    style: TextStyle,
    focus: FocusHandle,
    is_focused: bool,
    editor_state: EditorViewState,
}

impl DocumentView {
    pub fn new(
        core: Entity<Core>,
        input: Option<Entity<Input>>,
        view_id: ViewId,
        style: TextStyle,
        focus: &FocusHandle,
        is_focused: bool,
    ) -> Self {
        // Create viewport with placeholder document metrics (updated during render/paint).
        let line_height = px(20.0); // Default, will be updated
        let editor_state = EditorViewState::new(line_height, px(8.0));

        Self {
            core,
            input,
            view_id,
            style,
            focus: focus.clone(),
            is_focused,
            editor_state,
        }
    }

    pub fn set_focused(&mut self, is_focused: bool) -> bool {
        let changed = self.is_focused != is_focused;
        self.is_focused = is_focused;
        changed
    }

    pub fn update_text_style(&mut self, style: TextStyle) {
        // Recalculate line height with new font size
        // Use the actual font size as rem base for proper line height calculation
        self.editor_state.update_line_height_from_text_style(&style);
        self.style = style;
        self.editor_state.clear_shaped_lines_cache();
    }

    pub fn clear_shaped_lines_cache(&self) {
        self.editor_state.clear_shaped_lines_cache();
    }

    pub fn request_cursor_reveal(&self) {
        self.editor_state
            .request_cursor_reveal(EditorCursorReveal::Scrolloff);
    }

    pub fn request_cursor_center(&self) {
        self.editor_state
            .request_cursor_reveal(EditorCursorReveal::Center);
    }

    pub fn apply_viewport_scroll(
        &self,
        request: nucleotide_editor::EditorViewportScrollRequest,
    ) -> ViewportScrollUpdate {
        self.editor_state.apply_viewport_scroll(request)
    }

    pub fn visible_visual_rows(&self) -> usize {
        self.editor_state.visible_visual_rows()
    }

    pub fn top_visual_row(&self) -> usize {
        self.editor_state.top_visual_row()
    }

    pub fn content_visual_rows(&self) -> usize {
        self.editor_state.content_visual_rows()
    }

    pub fn layout_snapshot(&self) -> EditorViewLayoutSnapshot {
        self.editor_state.layout_snapshot()
    }
}

impl EventEmitter<DismissEvent> for DocumentView {}

impl Render for DocumentView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let runnable_tasks_by_line = runnable_tasks_by_line(&self.core, self.view_id, cx);
        let layout_snapshot = self.editor_state.layout_snapshot();
        let desired_gutter_extra_columns = if runnable_tasks_by_line.is_empty() {
            0
        } else {
            run_gutter_extra_columns(layout_snapshot.line_height, layout_snapshot.cell_width)
        };
        self.editor_state
            .set_gutter_extra_columns(desired_gutter_extra_columns);
        self.editor_state
            .set_gutter_run_button_lines(runnable_tasks_by_line.keys().copied());

        let editor_content = {
            let core = self.core.clone();
            let view_id = self.view_id;
            let style = self.style.clone();
            let focus = self.focus.clone();
            let paint_focus = focus.clone();
            let is_focused = self.is_focused;
            let input = self.input.clone();
            let scrollbar_thumb_color = cx.ui_theme().tokens.chrome.text_on_chrome;

            let mut editor_content = NativeEditorView::new(
                cx.entity_id(),
                self.editor_state.clone(),
                style.clone(),
                move |editor_state, bounds, after_layout, window, cx| {
                    paint_document_content(DocumentPaintParams {
                        core: &core,
                        view_id,
                        style: &style,
                        focus: &paint_focus,
                        is_focused,
                        editor_state,
                        bounds,
                        layout: after_layout,
                        window,
                        cx,
                    })
                },
            )
            .scrollbar_thumb_color(scrollbar_thumb_color)
            .track_focus(focus.clone());

            if let Some(input) = input {
                editor_content = editor_content.on_key_down(move |ev, _window, cx| {
                    let key = crate::utils::translate_key(&ev.keystroke);
                    input.update(cx, |_, cx| {
                        cx.emit(InputEvent::Key(key));
                    });
                    true
                });
            }

            editor_content
                .on_cursor_overlay(|overlay_plan, cx| {
                    let layout_info = cx.global_mut::<crate::overlay::WorkspaceLayoutInfo>();
                    if let Some(overlay_plan) = overlay_plan {
                        layout_info.cursor_position = Some(overlay_plan.cursor_position);
                        layout_info.cursor_size = Some(overlay_plan.cursor_size);
                    } else {
                        layout_info.cursor_position = None;
                        layout_info.cursor_size = None;
                    }
                })
                .on_pointer_selection({
                    let core = self.core.clone();
                    let view_id = self.view_id;
                    let editor_state = self.editor_state.clone();
                    let runnable_tasks_by_line = runnable_tasks_by_line.clone();

                    move |phase, event, cx| {
                        if phase == EditorPointerSelectionPhase::Begin
                            && let Some(task) = editor_state
                                .gutter_run_button_line_at(event.position)
                                .and_then(|line| runnable_tasks_by_line.get(&line).cloned())
                        {
                            run_gutter_task(&core, view_id, task, cx);
                            return;
                        }

                        handle_editor_pointer_selection(
                            &core,
                            view_id,
                            &editor_state,
                            phase,
                            event,
                            cx,
                        );
                    }
                })
                .on_mouse_down({
                    let core = self.core.clone();
                    let view_id = self.view_id;

                    move |_event, cx| {
                        focus_editor_view(&core, view_id, cx);
                    }
                })
        };

        div()
            .id(SharedString::from(format!("doc-view-{:?}", self.view_id)))
            .w_full()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .child(editor_content)
    }
}

fn runnable_tasks_by_line(
    core: &Entity<Core>,
    view_id: ViewId,
    cx: &mut Context<DocumentView>,
) -> BTreeMap<usize, ResolvedTask> {
    let document = {
        let core = core.read(cx);
        let Some(view) = core.editor.tree.try_get(view_id) else {
            return BTreeMap::new();
        };
        let Some(doc) = core.editor.documents.get(&view.doc) else {
            return BTreeMap::new();
        };
        let Some(path) = doc.path().cloned() else {
            return BTreeMap::new();
        };

        crate::runnables::RunnableDocument {
            path,
            text: String::from(doc.text().slice(..)),
            cursor_line: 0,
            project_root: None,
        }
    };

    crate::runnables::discover_local_rust_runnables(&document)
        .into_iter()
        .filter(|task| !crate::runnables::is_file_tests_runnable(task))
        .filter_map(|task| {
            let source = task.source()?;
            Some((source.line, task))
        })
        .collect()
}

impl Focusable for DocumentView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

struct DocumentPaintParams<'a> {
    core: &'a Entity<Core>,
    view_id: ViewId,
    style: &'a TextStyle,
    focus: &'a FocusHandle,
    is_focused: bool,
    editor_state: &'a mut EditorViewState,
    bounds: Bounds<Pixels>,
    layout: &'a mut EditorLayout,
    window: &'a mut Window,
    cx: &'a mut App,
}

fn paint_document_content(
    params: DocumentPaintParams<'_>,
) -> Option<nucleotide_editor::CursorOverlayPlan> {
    let DocumentPaintParams {
        core,
        view_id,
        style,
        focus,
        is_focused,
        editor_state,
        bounds,
        layout,
        window,
        cx,
    } = params;

    let helix_theme = cx.global::<crate::ThemeManager>().helix_theme().clone();
    core.update(cx, |core, cx| {
        let tokens = cx.theme().tokens;
        let ui_tokens = cx.ui_theme().tokens;
        let theme_styles = NativeEditorFrameThemeStyles::from_style_fn(|key| cx.theme_style(key));
        render_native_editor_frame(
            window,
            cx,
            NativeEditorFrameRenderParams {
                editor: &mut core.editor,
                view_id,
                editor_state,
                theme: &helix_theme,
                bounds,
                layout,
                text_style: style,
                font_size: style.font_size.to_pixels(px(16.0)),
                is_focused,
                focus,
                soft_wrap_minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
                theme_styles,
                palette: NativeEditorFramePalette {
                    fg_color: tokens.editor.text_primary,
                    bg_color: tokens.editor.background,
                    selection_primary: tokens.editor.selection_primary,
                    selection_secondary: tokens.editor.selection_secondary,
                    fallback_gutter_color: ui_tokens.editor.line_number,
                    diagnostic_highlight_base: tokens.chrome.text_on_chrome,
                    fallback_ruler_color: ui_tokens.chrome.border_default,
                    run_button_color: tokens.editor.success,
                },
            },
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;
    use nucleotide_editor::run_gutter_button_left;

    #[test]
    fn run_gutter_extra_columns_tracks_cell_width() {
        assert_eq!(run_gutter_extra_columns(px(20.0), px(12.0)), 2);
        assert_eq!(run_gutter_extra_columns(px(20.0), px(8.0)), 3);
    }

    #[test]
    fn run_gutter_button_is_centered_in_reserved_width() {
        assert_eq!(
            run_gutter_button_left(px(100.0), px(24.0), px(14.0)),
            px(81.0)
        );
    }
}
