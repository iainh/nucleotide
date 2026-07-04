// ABOUTME: GPUI-native text input field for non-editor UI surfaces
// ABOUTME: Owns editing state, selection, clipboard actions, and IME input hooks

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId,
    InteractiveElement, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement, Pixels, Point, Render, ShapedLine,
    SharedString, Style, Styled, TextRun, UTF16Selection, UnderlineStyle, Window, div, fill, point,
    prelude::FluentBuilder, px, relative, size,
};

use crate::actions::text_input::{
    Backspace, Cancel, Copy, Cut, Delete, MoveLeft, MoveRight, MoveToEnd, MoveToStart, Paste,
    SelectAll, SelectLeft, SelectRight, Submit,
};
use crate::{InputSize, InputVariant};

pub(crate) const TEXT_INPUT_CONTEXT: &str = "TextInput";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("delete", Delete, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("left", MoveLeft, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("right", MoveRight, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-a", SelectAll, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-v", Paste, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-c", Copy, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-x", Cut, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("home", MoveToStart, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("end", MoveToEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("enter", Submit, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(TEXT_INPUT_CONTEXT)),
    ]);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputEvent {
    Changed(SharedString),
    Submitted(SharedString),
    Cancelled,
}

pub struct TextInput {
    id: ElementId,
    variant: InputVariant,
    size: InputSize,
    disabled: bool,
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    error: Option<SharedString>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl TextInput {
    pub fn new(id: impl Into<ElementId>, cx: &mut Context<Self>) -> Self {
        Self {
            id: id.into(),
            variant: InputVariant::Default,
            size: InputSize::Medium,
            disabled: false,
            focus_handle: cx.focus_handle().tab_stop(true),
            content: SharedString::default(),
            placeholder: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            error: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
        }
    }

    pub fn variant(mut self, variant: InputVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: InputSize) -> Self {
        self.size = size;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn value(&self) -> SharedString {
        self.content.clone()
    }

    pub fn set_value(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = value.into();
        let cursor = self.content.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.last_bounds = None;
        cx.emit(TextInputEvent::Changed(self.content.clone()));
        cx.notify();
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self
    }

    pub fn clear_error(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        cx.notify();
    }

    pub fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn move_to_start(&mut self, _: &MoveToStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn move_to_end(&mut self, _: &MoveToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }

        if self.selected_range.is_empty() {
            let previous = self.previous_boundary(self.cursor_offset());
            if previous == self.cursor_offset() {
                window.play_system_bell();
                return;
            }
            self.select_to(previous, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }

        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if next == self.cursor_offset() {
                window.play_system_bell();
                return;
            }
            self.select_to(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }

        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }

        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Submitted(self.content.clone()));
        cx.stop_propagation();
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Cancelled);
        cx.stop_propagation();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window, cx);
        self.is_selecting = true;

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx);
        }
        cx.stop_propagation();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = false;
        cx.stop_propagation();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting && event.dragging() {
            self.select_to(self.index_for_mouse_position(event.position), cx);
            cx.stop_propagation();
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.clamp_to_boundary(offset);
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.clamp_to_boundary(offset);
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return self.cursor_offset();
        };

        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn clamp_to_boundary(&self, offset: usize) -> usize {
        if offset >= self.content.len() {
            self.content.len()
        } else if self.content.is_char_boundary(offset) {
            offset
        } else {
            self.content
                .char_indices()
                .map(|(index, _)| index)
                .take_while(|index| *index < offset)
                .last()
                .unwrap_or(0)
        }
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        offset_from_utf16(self.content.as_ref(), offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        offset_to_utf16(self.content.as_ref(), offset)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn reset_layout_cache(&mut self) {
        self.last_layout = None;
        self.last_bounds = None;
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }

        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.reset_layout_cache();
        cx.emit(TextInputEvent::Changed(self.content.clone()));
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }

        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();

        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + new_text.len());
        }

        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| {
                range.start + offset_from_utf16(new_text, range_utf16.start)
                    ..range.start + offset_from_utf16(new_text, range_utf16.end)
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        self.reset_layout_cache();
        cx.emit(TextInputEvent::Changed(self.content.clone()));
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        !self.disabled
    }
}

struct TextInputElement {
    input: Entity<TextInput>,
    text_color: gpui::Hsla,
    placeholder_color: gpui::Hsla,
    selection_color: gpui::Hsla,
    cursor_color: gpui::Hsla,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextInputElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor_offset = input.cursor_offset();
        let display_text = if content.is_empty() {
            input.placeholder.clone()
        } else {
            content.clone()
        };
        let text_color = if content.is_empty() {
            self.placeholder_color
        } else {
            self.text_color
        };

        let run = TextRun {
            len: display_text.len(),
            font: window.text_style().font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len().saturating_sub(marked_range.end),
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_x = line.x_for_index(cursor_offset);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_x, bounds.top()),
                        size(px(1.0), bounds.bottom() - bounds.top()),
                    ),
                    self.cursor_color,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    self.selection_color,
                )),
                None,
            )
        };

        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }

        let line = prepaint
            .line
            .take()
            .expect("text line should be prepainted");
        line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        )
        .ok();

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<crate::Theme>();
        let input_tokens = theme.tokens.input_tokens();
        let inset_highlight = theme.tokens.chrome.inset_highlight;
        let inset_shadow = theme.tokens.chrome.inset_shadow;

        let is_focused = self.focus_handle.is_focused(window);
        let has_error = self.error.is_some();
        let text_size = match self.size {
            InputSize::Small => theme.tokens.sizes.text_sm,
            InputSize::Medium => theme.tokens.sizes.text_md,
            InputSize::Large => theme.tokens.sizes.text_lg,
        };
        let background = if self.disabled {
            input_tokens.background_disabled
        } else if self.variant == InputVariant::Ghost {
            input_tokens.background.alpha(0.0)
        } else if is_focused {
            input_tokens.background_focus
        } else {
            input_tokens.background
        };
        let border = if self.disabled {
            input_tokens.border_disabled
        } else if has_error {
            input_tokens.border_error
        } else if is_focused {
            input_tokens.border_focus
        } else {
            input_tokens.border
        };

        div()
            .id(self.id.clone())
            .key_context(TEXT_INPUT_CONTEXT)
            .track_focus(&self.focus_handle)
            .tab_stop(true)
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .min_w(px(0.0))
            .bg(background)
            .border_1()
            .border_color(border)
            .rounded_md()
            .px_2()
            .py_1()
            .text_size(text_size)
            .text_color(if self.disabled {
                input_tokens.text_disabled
            } else {
                input_tokens.text
            })
            .cursor(CursorStyle::IBeam)
            .when(!is_focused && !has_error, |this| {
                this.shadow(vec![
                    inset_shadow.to_box_shadow(true),
                    inset_highlight.to_box_shadow(true),
                ])
            })
            .when(is_focused && !has_error, |this| {
                this.shadow(vec![
                    gpui::BoxShadow {
                        color: input_tokens.focus_ring,
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: px(2.0),
                        inset: false,
                    },
                    inset_highlight.to_box_shadow(true),
                ])
            })
            .when(has_error, |this| {
                this.shadow(vec![
                    gpui::BoxShadow {
                        color: input_tokens.border_error,
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: px(2.0),
                        inset: false,
                    },
                    inset_shadow.to_box_shadow(true),
                ])
            })
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::move_to_start))
            .on_action(cx.listener(Self::move_to_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(TextInputElement {
                        input: cx.entity().clone(),
                        text_color: input_tokens.text,
                        placeholder_color: input_tokens.placeholder,
                        selection_color: input_tokens.focus_ring.alpha(0.35),
                        cursor_color: input_tokens.text,
                    }),
            )
    }
}

fn offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf8_offset = 0;
    let mut utf16_count = 0;

    for ch in text.chars() {
        if utf16_count >= offset {
            break;
        }
        utf16_count += ch.len_utf16();
        utf8_offset += ch.len_utf8();
    }

    utf8_offset
}

fn offset_to_utf16(text: &str, offset: usize) -> usize {
    let mut utf16_offset = 0;
    let mut utf8_count = 0;

    for ch in text.chars() {
        if utf8_count >= offset {
            break;
        }
        utf8_count += ch.len_utf8();
        utf16_offset += ch.len_utf16();
    }

    utf16_offset
}

#[cfg(test)]
mod tests {
    use gpui::{
        AppContext as _, Context, Entity, Focusable, IntoElement, ParentElement as _, Render,
        Styled as _, TestAppContext, Window, div, px,
    };

    use super::*;

    struct TextInputHarness {
        input: Entity<TextInput>,
    }

    impl TextInputHarness {
        fn new(cx: &mut Context<Self>) -> Self {
            let input = cx.new(|cx| TextInput::new("test-input", cx).placeholder("Type here"));
            Self { input }
        }
    }

    impl Render for TextInputHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(div().w(px(240.0)).child(self.input.clone()))
        }
    }

    fn init_theme(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(crate::Theme::from_tokens(crate::DesignTokens::dark()));
            init(cx);
        });
    }

    #[gpui::test]
    fn replace_text_updates_value_and_selection(cx: &mut TestAppContext) {
        init_theme(cx);
        let (harness, cx) = cx.add_window_view(|_, cx| TextInputHarness::new(cx));
        let input = harness.read_with(cx, |harness, _| harness.input.clone());

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.replace_text_in_range(None, "hello", window, cx);
            });
        });

        input.read_with(cx, |input, _| {
            assert_eq!(input.value().as_ref(), "hello");
            assert_eq!(input.selected_range, 5..5);
        });
    }

    #[gpui::test]
    fn actions_edit_focused_text(cx: &mut TestAppContext) {
        init_theme(cx);
        let (harness, cx) = cx.add_window_view(|_, cx| TextInputHarness::new(cx));
        let input = harness.read_with(cx, |harness, _| harness.input.clone());
        let focus = input.read_with(cx, |input, cx| input.focus_handle(cx));

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.replace_text_in_range(None, "abc", window, cx);
            });
            window.focus(&focus, cx);
            focus.dispatch_action(&MoveLeft, window, cx);
            focus.dispatch_action(&Backspace, window, cx);
        });

        input.read_with(cx, |input, _| {
            assert_eq!(input.value().as_ref(), "ac");
            assert_eq!(input.selected_range, 1..1);
        });
    }

    #[gpui::test]
    fn select_all_and_cut_use_clipboard(cx: &mut TestAppContext) {
        init_theme(cx);
        let (harness, cx) = cx.add_window_view(|_, cx| TextInputHarness::new(cx));
        let input = harness.read_with(cx, |harness, _| harness.input.clone());
        let focus = input.read_with(cx, |input, cx| input.focus_handle(cx));

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.replace_text_in_range(None, "value", window, cx);
            });
            window.focus(&focus, cx);
            focus.dispatch_action(&SelectAll, window, cx);
            focus.dispatch_action(&Cut, window, cx);
        });

        input.read_with(cx, |input, _| {
            assert_eq!(input.value().as_ref(), "");
            assert_eq!(input.selected_range, 0..0);
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("value".to_string())
        );
    }

    #[test]
    fn utf16_offsets_round_trip_non_ascii_text() {
        let text = "a💡b";

        assert_eq!(offset_to_utf16(text, 1), 1);
        assert_eq!(offset_to_utf16(text, "a💡".len()), 3);
        assert_eq!(offset_from_utf16(text, 3), "a💡".len());
    }
}
