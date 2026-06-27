use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Bounds, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, TextStyle, Window, div, px,
};
// Import helix's syntax highlighting system
use helix_view::{DocumentId, ViewId};
use nucleotide_events::v2::run::ResolvedTask;
use nucleotide_types::scrollbar::SCROLLBAR_THICKNESS;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::scrollbar::{Scrollbar, ScrollbarState};
use nucleotide_ui::theme_manager::HelixThemedContext;
use nucleotide_ui::{Button, ButtonSize, ButtonVariant, MarkdownStyle, Tooltipped, markdown};

use crate::{Core, Input, InputEvent};
use nucleotide_editor::{
    DiagnosticSeverityIconColors, EDITOR_MINIMUM_VIEWPORT_COLUMNS, EditorCursorReveal,
    EditorLayout, EditorPointerSelectionPhase, EditorSurfacePointerEvent, EditorViewLayoutSnapshot,
    EditorViewState, NativeEditorFramePalette, NativeEditorFrameRenderParams,
    NativeEditorFrameThemeStyles, NativeEditorView, ViewportScrollUpdate,
    log_pointer_selection_outcome, render_native_editor_frame, run_gutter_extra_columns,
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
    markdown_modes: BTreeMap<DocumentId, MarkdownDisplayMode>,
    markdown_scroll_handle: gpui::ScrollHandle,
    markdown_scrollbar_state: ScrollbarState,
    runnable_tasks_cache: Option<RunnableTasksCache>,
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
        let markdown_scroll_handle = gpui::ScrollHandle::new();
        let markdown_scrollbar_state = ScrollbarState::new(markdown_scroll_handle.clone());

        Self {
            core,
            input,
            view_id,
            style,
            focus: focus.clone(),
            is_focused,
            editor_state,
            markdown_modes: BTreeMap::new(),
            markdown_scroll_handle,
            markdown_scrollbar_state,
            runnable_tasks_cache: None,
        }
    }

    pub fn set_focused(&mut self, is_focused: bool) -> bool {
        let changed = self.is_focused != is_focused;
        self.is_focused = is_focused;
        changed
    }

    pub fn update_text_style(&mut self, style: TextStyle) -> bool {
        if self.style == style {
            return false;
        }

        let metrics_changed = text_style_metrics_changed(&self.style, &style);
        let shape_changed = text_style_shape_changed(&self.style, &style);

        if metrics_changed {
            self.editor_state.update_line_height_from_text_style(&style);
        }

        self.style = style;

        if shape_changed {
            self.editor_state.clear_shaped_lines_cache();
        }

        true
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

    fn markdown_mode_for(&self, doc_id: DocumentId) -> MarkdownDisplayMode {
        self.markdown_modes
            .get(&doc_id)
            .copied()
            .unwrap_or_default()
    }

    fn set_markdown_mode(&mut self, doc_id: DocumentId, mode: MarkdownDisplayMode) -> bool {
        let current = self.markdown_mode_for(doc_id);
        if current == mode {
            return false;
        }

        if mode == MarkdownDisplayMode::default() {
            self.markdown_modes.remove(&doc_id);
        } else {
            self.markdown_modes.insert(doc_id, mode);
        }

        true
    }

    fn runnable_tasks_by_line(&mut self, cx: &mut Context<Self>) -> BTreeMap<usize, ResolvedTask> {
        let Some(snapshot) = runnable_document_snapshot(&self.core, self.view_id, cx) else {
            self.runnable_tasks_cache = None;
            return BTreeMap::new();
        };

        if self
            .runnable_tasks_cache
            .as_ref()
            .is_some_and(|cache| cache.matches_snapshot(&snapshot))
        {
            return self
                .runnable_tasks_cache
                .as_ref()
                .map(|cache| cache.tasks_by_line.clone())
                .unwrap_or_default();
        }

        let document = {
            let core = self.core.read(cx);
            let Some(doc) = core.editor.documents.get(&snapshot.doc_id) else {
                self.runnable_tasks_cache = None;
                return BTreeMap::new();
            };

            crate::runnables::RunnableDocument {
                path: snapshot.path.clone(),
                text: String::from(doc.text().slice(..)),
                cursor_line: 0,
                project_root: None,
            }
        };

        let tasks_by_line = crate::runnables::discover_local_rust_runnables(&document)
            .into_iter()
            .filter(|task| !crate::runnables::is_file_tests_runnable(task))
            .filter_map(|task| {
                let source = task.source()?;
                Some((source.line, task))
            })
            .collect::<BTreeMap<_, _>>();

        self.runnable_tasks_cache = Some(RunnableTasksCache {
            doc_id: snapshot.doc_id,
            version: snapshot.version,
            path: snapshot.path,
            tasks_by_line: tasks_by_line.clone(),
        });

        tasks_by_line
    }
}

fn text_style_metrics_changed(previous: &TextStyle, next: &TextStyle) -> bool {
    previous.font_size != next.font_size || previous.line_height != next.line_height
}

fn text_style_shape_changed(previous: &TextStyle, next: &TextStyle) -> bool {
    previous.font_family != next.font_family
        || previous.font_features != next.font_features
        || previous.font_fallbacks != next.font_fallbacks
        || previous.font_size != next.font_size
        || previous.font_weight != next.font_weight
        || previous.font_style != next.font_style
}

#[derive(Clone)]
struct RunnableDocumentSnapshot {
    doc_id: DocumentId,
    version: i32,
    path: PathBuf,
}

struct RunnableTasksCache {
    doc_id: DocumentId,
    version: i32,
    path: PathBuf,
    tasks_by_line: BTreeMap<usize, ResolvedTask>,
}

impl RunnableTasksCache {
    fn matches_snapshot(&self, snapshot: &RunnableDocumentSnapshot) -> bool {
        self.doc_id == snapshot.doc_id
            && self.version == snapshot.version
            && self.path == snapshot.path
    }
}

impl EventEmitter<DismissEvent> for DocumentView {}

impl Render for DocumentView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let runnable_tasks_by_line = self.runnable_tasks_by_line(cx);
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

        let markdown_document = markdown_document_info(&self.core, self.view_id, cx);
        let markdown_mode = markdown_document
            .as_ref()
            .map(|snapshot| self.markdown_mode_for(snapshot.doc_id))
            .unwrap_or_default();
        let show_rendered_markdown =
            matches!(markdown_mode, MarkdownDisplayMode::Rendered) && markdown_document.is_some();

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
                        cx.emit(InputEvent::key_down(key, ev.is_held));
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

        let rendered_markdown = if show_rendered_markdown {
            markdown_document_snapshot(&self.core, self.view_id, cx)
        } else {
            None
        };

        let content = if let Some(snapshot) = rendered_markdown.as_ref() {
            self.render_markdown_document(snapshot, cx)
        } else {
            editor_content.into_any_element()
        };

        let controls = markdown_document
            .as_ref()
            .map(|snapshot| self.render_markdown_controls(snapshot.doc_id, markdown_mode, cx));

        div()
            .id(SharedString::from(format!("doc-view-{:?}", self.view_id)))
            .w_full()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .child(content)
            .when_some(controls, gpui::ParentElement::child)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MarkdownDisplayMode {
    #[default]
    Source,
    Rendered,
}

struct MarkdownDocumentSnapshot {
    doc_id: DocumentId,
    source: SharedString,
}

struct MarkdownDocumentInfo {
    doc_id: DocumentId,
}

fn markdown_document_info(
    core: &Entity<Core>,
    view_id: ViewId,
    cx: &mut Context<DocumentView>,
) -> Option<MarkdownDocumentInfo> {
    let core = core.read(cx);
    let view = core.editor.tree.try_get(view_id)?;
    let doc = core.editor.documents.get(&view.doc)?;
    if !doc
        .path()
        .is_some_and(|path| is_markdown_document_path(path))
    {
        return None;
    }

    Some(MarkdownDocumentInfo { doc_id: view.doc })
}

fn markdown_document_snapshot(
    core: &Entity<Core>,
    view_id: ViewId,
    cx: &mut Context<DocumentView>,
) -> Option<MarkdownDocumentSnapshot> {
    let core = core.read(cx);
    let view = core.editor.tree.try_get(view_id)?;
    let doc = core.editor.documents.get(&view.doc)?;
    if !doc
        .path()
        .is_some_and(|path| is_markdown_document_path(path))
    {
        return None;
    }

    Some(MarkdownDocumentSnapshot {
        doc_id: view.doc,
        source: SharedString::from(String::from(doc.text().slice(..))),
    })
}

fn is_markdown_document_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "mdown" | "mkd" | "mkdn"
            )
        })
}

impl DocumentView {
    fn render_markdown_document(
        &self,
        snapshot: &MarkdownDocumentSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = &cx.theme().tokens;
        let editor_font = cx.global::<crate::types::EditorFontConfig>();
        let mut markdown_style = MarkdownStyle::preview_from_tokens(tokens);
        markdown_style.code_font_family = SharedString::from(editor_font.family.clone());
        let focus = self.focus.clone();
        let click_focus = focus.clone();
        let core = self.core.clone();
        let view_id = self.view_id;
        let scroll_content = div()
            .id(SharedString::from(format!(
                "markdown-rendered-content-{:?}",
                snapshot.doc_id
            )))
            .size_full()
            .min_h(px(0.0))
            .focusable()
            .track_focus(&focus)
            .overflow_y_scroll()
            .track_scroll(&self.markdown_scroll_handle)
            .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                focus_editor_view(&core, view_id, cx);
                window.focus(&click_focus, cx);
            })
            .px(tokens.sizes.space_8)
            .py(tokens.sizes.space_8)
            .child(markdown(snapshot.source.clone(), markdown_style));

        div()
            .id(SharedString::from(format!(
                "markdown-rendered-{:?}",
                snapshot.doc_id
            )))
            .relative()
            .size_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(scroll_content)
            .when_some(
                Scrollbar::vertical(self.markdown_scrollbar_state.clone()),
                |container, scrollbar| {
                    container.child(
                        div()
                            .id(SharedString::from(format!(
                                "markdown-rendered-scrollbar-{:?}",
                                snapshot.doc_id
                            )))
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .w(SCROLLBAR_THICKNESS)
                            .child(scrollbar),
                    )
                },
            )
            .into_any_element()
    }

    fn render_markdown_controls(
        &self,
        doc_id: DocumentId,
        mode: MarkdownDisplayMode,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = &cx.theme().tokens;
        let source_variant = markdown_mode_button_variant(mode, MarkdownDisplayMode::Source);
        let rendered_variant = markdown_mode_button_variant(mode, MarkdownDisplayMode::Rendered);
        let view = cx.entity().clone();
        let focus = self.focus.clone();
        let source_view = view.clone();
        let source_focus = focus.clone();
        let rendered_view = view;
        let rendered_focus = focus;

        div()
            .id(SharedString::from(format!(
                "markdown-mode-controls-{doc_id}"
            )))
            .absolute()
            .top(px(10.0))
            .right(px(14.0))
            .flex()
            .items_center()
            .gap(tokens.sizes.space_1)
            .p(tokens.sizes.space_1)
            .rounded(tokens.sizes.radius_md)
            .bg(nucleotide_ui::tokens::with_alpha(
                tokens.chrome.surface,
                0.58,
            ))
            .border_1()
            .border_color(nucleotide_ui::tokens::with_alpha(
                tokens.chrome.border_muted,
                0.64,
            ))
            .child(
                Button::icon_only(
                    SharedString::from(format!("markdown-source-{doc_id}")),
                    "icons/code.svg",
                )
                .variant(source_variant)
                .size(ButtonSize::Small)
                .tooltip("Show Source")
                .activate_on_mouse_down()
                .on_click({
                    move |_event, window, cx| {
                        source_view.update(cx, |view, cx| {
                            if view.set_markdown_mode(doc_id, MarkdownDisplayMode::Source) {
                                cx.notify();
                            }
                        });
                        window.focus(&source_focus, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(
                Button::icon_only(
                    SharedString::from(format!("markdown-rendered-{doc_id}")),
                    "icons/book-text.svg",
                )
                .variant(rendered_variant)
                .size(ButtonSize::Small)
                .tooltip("Render Markdown")
                .activate_on_mouse_down()
                .on_click({
                    move |_event, window, cx| {
                        rendered_view.update(cx, |view, cx| {
                            if view.set_markdown_mode(doc_id, MarkdownDisplayMode::Rendered) {
                                cx.notify();
                            }
                        });
                        window.focus(&rendered_focus, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .into_any_element()
    }
}

fn markdown_mode_button_variant(
    current: MarkdownDisplayMode,
    button_mode: MarkdownDisplayMode,
) -> ButtonVariant {
    if current == button_mode {
        ButtonVariant::Secondary
    } else {
        ButtonVariant::Ghost
    }
}

fn runnable_document_snapshot(
    core: &Entity<Core>,
    view_id: ViewId,
    cx: &mut Context<DocumentView>,
) -> Option<RunnableDocumentSnapshot> {
    let core = core.read(cx);
    let view = core.editor.tree.try_get(view_id)?;
    let doc = core.editor.documents.get(&view.doc)?;
    let path = doc.path()?.to_path_buf();

    if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
        return None;
    }

    Some(RunnableDocumentSnapshot {
        doc_id: view.doc,
        version: doc.version(),
        path,
    })
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
                    diagnostic_icon_colors: DiagnosticSeverityIconColors {
                        error: tokens.editor.diagnostic_error,
                        warning: tokens.editor.diagnostic_warning,
                        info: tokens.editor.diagnostic_info,
                        hint: tokens.editor.diagnostic_hint,
                    },
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

    #[test]
    fn markdown_document_path_detection_accepts_common_extensions() {
        for path in [
            Path::new("README.md"),
            Path::new("guide.MARKDOWN"),
            Path::new("notes.mdown"),
            Path::new("draft.mkd"),
            Path::new("chapter.mkdn"),
        ] {
            assert!(
                is_markdown_document_path(path),
                "expected {} to be detected as markdown",
                path.display()
            );
        }
    }

    #[test]
    fn markdown_document_path_detection_rejects_non_markdown_files() {
        for path in [
            Path::new("main.rs"),
            Path::new("markdown.txt"),
            Path::new("README"),
            Path::new("archive.md.bak"),
        ] {
            assert!(
                !is_markdown_document_path(path),
                "expected {} to remain a source editor document",
                path.display()
            );
        }
    }

    #[test]
    fn markdown_mode_button_variant_marks_current_mode() {
        assert_eq!(
            markdown_mode_button_variant(MarkdownDisplayMode::Source, MarkdownDisplayMode::Source),
            ButtonVariant::Secondary
        );
        assert_eq!(
            markdown_mode_button_variant(
                MarkdownDisplayMode::Source,
                MarkdownDisplayMode::Rendered
            ),
            ButtonVariant::Ghost
        );
        assert_eq!(
            markdown_mode_button_variant(
                MarkdownDisplayMode::Rendered,
                MarkdownDisplayMode::Rendered
            ),
            ButtonVariant::Secondary
        );
    }
}
