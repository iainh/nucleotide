use std::{cell::Cell, rc::Rc};

use gpui::{
    App, Bounds, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Render, SharedString,
    Styled, TextStyle, Window, div, px,
};
// Import helix's syntax highlighting system
use helix_view::{DocumentId, ViewId, document::Mode};
use nucleotide_logging::debug;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::theme_manager::HelixThemedContext;

use crate::{Core, Input, InputEvent};
use nucleotide_editor::{
    EDITOR_MINIMUM_VIEWPORT_COLUMNS, EditorCursorReveal, EditorLayout, EditorPointerSelectionPhase,
    EditorSurfacePointerEvent, EditorViewContentPrepareParams, EditorViewLayoutSnapshot,
    EditorViewState, NativeEditorFramePalette, NativeEditorFrameRenderParams,
    NativeEditorFrameThemeStyles, NativeEditorView, log_pointer_selection_outcome,
    render_native_editor_frame,
};

fn handle_editor_pointer_selection(
    core: &Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    editor_state: &EditorViewState,
    phase: EditorPointerSelectionPhase,
    event: EditorSurfacePointerEvent,
    cx: &mut App,
) {
    let outcome = core.update(cx, |core, cx| {
        let outcome = editor_state.handle_pointer_selection_outcome(
            &mut core.editor,
            doc_id,
            view_id,
            phase,
            event,
        );

        if outcome.changed() {
            cx.notify();
        }

        outcome
    });

    log_pointer_selection_outcome(outcome);
}

fn should_bubble_to_workspace_leader(
    core: &Entity<Core>,
    leader_passthrough: &Cell<bool>,
    ev: &KeyDownEvent,
    cx: &mut App,
) -> bool {
    let is_normal_mode = core.read(cx).editor.mode() == Mode::Normal;
    if !is_normal_mode {
        leader_passthrough.set(false);
        return false;
    }

    let key = ev.keystroke.key.as_str();
    if !leader_passthrough.get() {
        if matches!(key, "space" | " ") && ev.keystroke.modifiers.number_of_modifiers() == 0 {
            leader_passthrough.set(true);
            return true;
        }

        return false;
    }

    leader_passthrough.set(false);
    true
}

pub struct DocumentView {
    core: Entity<Core>,
    input: Option<Entity<Input>>,
    view_id: ViewId,
    style: TextStyle,
    focus: FocusHandle,
    is_focused: bool,
    editor_state: EditorViewState,
    leader_passthrough: Rc<Cell<bool>>,
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
            leader_passthrough: Rc::new(Cell::new(false)),
        }
    }

    pub fn set_focused(&mut self, is_focused: bool) {
        self.is_focused = is_focused;
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

    pub fn layout_snapshot(&self) -> EditorViewLayoutSnapshot {
        self.editor_state.layout_snapshot()
    }
}

impl EventEmitter<DismissEvent> for DocumentView {}

impl Render for DocumentView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // DocumentView render creates the native editor element for actual painting.
        let Some(content_state) = ({
            let core = self.core.read(cx);
            let editor = &core.editor;
            let theme = cx.global::<crate::ThemeManager>().helix_theme().clone();
            self.editor_state
                .prepare_content_for_render(EditorViewContentPrepareParams {
                    editor,
                    view_id: self.view_id,
                    theme: Some(&theme),
                    text_system: cx.text_system(),
                    text_style: &self.style,
                })
        }) else {
            return div().id(SharedString::from(format!("doc-view-{:?}", self.view_id)));
        };
        let doc_id = content_state.doc_id;
        debug!(
            physical_lines = content_state.physical_lines,
            visual_rows = content_state.update.visual_rows,
            soft_wrap = content_state.update.soft_wrap,
            "Primed native editor viewport content metrics"
        );

        let editor_content = {
            let core = self.core.clone();
            let view_id = self.view_id;
            let style = self.style.clone();
            let focus = self.focus.clone();
            let paint_focus = focus.clone();
            let is_focused = self.is_focused;
            let input = self.input.clone();
            let key_core = self.core.clone();
            let leader_passthrough = Rc::clone(&self.leader_passthrough);

            let mut editor_content = NativeEditorView::new(
                cx.entity_id(),
                self.editor_state.clone(),
                style.clone(),
                move |editor_state, bounds, after_layout, window, cx| {
                    paint_document_content(DocumentPaintParams {
                        core: &core,
                        doc_id,
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
            .track_focus(focus.clone());

            if let Some(input) = input {
                editor_content = editor_content.on_key_down(move |ev, _window, cx| {
                    if should_bubble_to_workspace_leader(&key_core, &leader_passthrough, ev, cx) {
                        return false;
                    }

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

                    move |phase, event, cx| {
                        handle_editor_pointer_selection(
                            &core,
                            doc_id,
                            view_id,
                            &editor_state,
                            phase,
                            event,
                            cx,
                        );
                    }
                })
        };

        div()
            .id(SharedString::from(format!("doc-view-{:?}", self.view_id)))
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .child(editor_content)
    }
}

impl Focusable for DocumentView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

struct DocumentPaintParams<'a> {
    core: &'a Entity<Core>,
    doc_id: DocumentId,
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
        doc_id,
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
                doc_id,
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
                },
            },
        )
    })
}
