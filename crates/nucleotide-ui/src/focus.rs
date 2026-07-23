// ABOUTME: Role-based focus registry for major application surfaces
// ABOUTME: Keeps focus restoration separate from legacy global input dispatch

use std::sync::{Arc, RwLock};

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, FocusHandle, InteractiveElement, IntoElement, KeyBinding, ParentElement,
    RenderOnce, Styled, Window, div,
};

use crate::actions::focus::{FocusNext, FocusPrevious};

pub const FOCUS_TRAVERSAL_CONTEXT: &str = "FocusTraversal";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, Some(FOCUS_TRAVERSAL_CONTEXT)),
        KeyBinding::new("shift-tab", FocusPrevious, Some(FOCUS_TRAVERSAL_CONTEXT)),
    ]);
}

/// Focus role registry for common UI areas.
///
/// Components can register and retrieve major FocusHandles without ad-hoc
/// storage. This is intentionally not a per-widget navigation system; ordinary
/// traversal should use GPUI tab stops or component-owned actions.
#[derive(Clone, Default)]
pub struct FocusCoordinator {
    editor: Arc<RwLock<Option<FocusHandle>>>,
    completion: Arc<RwLock<Option<FocusHandle>>>,
    prompt: Arc<RwLock<Option<FocusHandle>>>,
    terminal: Arc<RwLock<Option<FocusHandle>>>,
    picker: Arc<RwLock<Option<FocusHandle>>>,
    diagnostics: Arc<RwLock<Option<FocusHandle>>>,
    file_tree: Arc<RwLock<Option<FocusHandle>>>,
}

impl FocusCoordinator {
    pub fn set_editor_focus(&self, h: FocusHandle) {
        if let Ok(mut slot) = self.editor.write() {
            *slot = Some(h);
        }
    }

    pub fn editor_focus(&self) -> Option<FocusHandle> {
        self.editor.read().ok().and_then(|g| g.clone())
    }

    pub fn set_completion_focus(&self, h: FocusHandle) {
        if let Ok(mut slot) = self.completion.write() {
            *slot = Some(h);
        }
    }

    pub fn clear_completion_focus(&self) {
        if let Ok(mut slot) = self.completion.write() {
            *slot = None;
        }
    }

    pub fn completion_focus(&self) -> Option<FocusHandle> {
        self.completion.read().ok().and_then(|g| g.clone())
    }

    pub fn set_prompt_focus(&self, h: FocusHandle) {
        if let Ok(mut slot) = self.prompt.write() {
            *slot = Some(h);
        }
    }

    pub fn clear_prompt_focus(&self) {
        if let Ok(mut slot) = self.prompt.write() {
            *slot = None;
        }
    }

    pub fn prompt_focus(&self) -> Option<FocusHandle> {
        self.prompt.read().ok().and_then(|g| g.clone())
    }

    pub fn set_terminal_focus(&self, h: FocusHandle) {
        if let Ok(mut slot) = self.terminal.write() {
            *slot = Some(h);
        }
    }

    pub fn clear_terminal_focus(&self) {
        if let Ok(mut slot) = self.terminal.write() {
            *slot = None;
        }
    }

    pub fn clear_terminal_focus_if(&self, focus: &FocusHandle) {
        if let Ok(mut slot) = self.terminal.write()
            && slot.as_ref() == Some(focus)
        {
            *slot = None;
        }
    }

    pub fn terminal_focus(&self) -> Option<FocusHandle> {
        self.terminal.read().ok().and_then(|g| g.clone())
    }

    pub fn set_picker_focus(&self, h: FocusHandle) {
        if let Ok(mut slot) = self.picker.write() {
            *slot = Some(h);
        }
    }

    pub fn clear_picker_focus(&self) {
        if let Ok(mut slot) = self.picker.write() {
            *slot = None;
        }
    }

    pub fn picker_focus(&self) -> Option<FocusHandle> {
        self.picker.read().ok().and_then(|g| g.clone())
    }

    pub fn set_diagnostics_focus(&self, h: FocusHandle) {
        if let Ok(mut slot) = self.diagnostics.write() {
            *slot = Some(h);
        }
    }

    pub fn diagnostics_focus(&self) -> Option<FocusHandle> {
        self.diagnostics.read().ok().and_then(|g| g.clone())
    }

    pub fn set_file_tree_focus(&self, h: FocusHandle) {
        if let Ok(mut slot) = self.file_tree.write() {
            *slot = Some(h);
        }
    }

    pub fn file_tree_focus(&self) -> Option<FocusHandle> {
        self.file_tree.read().ok().and_then(|g| g.clone())
    }

    /// Focus the given role if a handle is registered. Returns true on success.
    pub fn focus_role(&self, window: &mut Window, cx: &mut App, role: FocusRole) -> bool {
        let handle = match role {
            FocusRole::Editor => self.editor_focus(),
            FocusRole::Completion => self.completion_focus(),
            FocusRole::Prompt => self.prompt_focus(),
            FocusRole::Terminal => self.terminal_focus(),
            FocusRole::Picker => self.picker_focus(),
            FocusRole::Diagnostics => self.diagnostics_focus(),
            FocusRole::FileTree => self.file_tree_focus(),
        };
        if let Some(h) = handle {
            if !h.is_focused(window) {
                h.focus(window, cx);
            }
            true
        } else {
            false
        }
    }

    /// Focus the first available role in order. Returns true if any focus was applied.
    pub fn focus_first(&self, window: &mut Window, cx: &mut App, roles: &[FocusRole]) -> bool {
        for role in roles {
            if self.focus_role(window, cx, *role) {
                return true;
            }
        }
        false
    }
}

impl gpui::Global for FocusCoordinator {}

/// Logical focus roles supported by the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusRole {
    Editor,
    Completion,
    Prompt,
    Terminal,
    Picker,
    Diagnostics,
    FileTree,
}

/// Wraps a focus scope with standard Tab and Shift-Tab traversal actions.
#[derive(IntoElement)]
pub struct FocusTraversal {
    child: AnyElement,
    key_context: Option<&'static str>,
}

impl FocusTraversal {
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            key_context: Some(FOCUS_TRAVERSAL_CONTEXT),
        }
    }

    pub fn key_context(mut self, key_context: &'static str) -> Self {
        self.key_context = Some(key_context);
        self
    }

    pub fn without_key_context(mut self) -> Self {
        self.key_context = None;
        self
    }
}

impl RenderOnce for FocusTraversal {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .when_some(self.key_context, |this, key_context| {
                this.key_context(key_context)
            })
            .size_full()
            .on_action(|_: &FocusNext, window, cx| {
                window.focus_next(cx);
                cx.stop_propagation();
            })
            .on_action(|_: &FocusPrevious, window, cx| {
                window.focus_prev(cx);
                cx.stop_propagation();
            })
            .child(self.child)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, ParentElement as _, Render, TestAppContext, div, px};

    use super::*;

    #[test]
    fn focus_coordinator_clear_methods_are_idempotent() {
        let coordinator = FocusCoordinator::default();

        coordinator.clear_picker_focus();
        coordinator.clear_prompt_focus();
        coordinator.clear_completion_focus();
        coordinator.clear_terminal_focus();

        assert!(coordinator.picker_focus().is_none());
        assert!(coordinator.prompt_focus().is_none());
        assert!(coordinator.completion_focus().is_none());
        assert!(coordinator.terminal_focus().is_none());
    }

    struct FocusTraversalHarness {
        first: FocusHandle,
        second: FocusHandle,
    }

    impl FocusTraversalHarness {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                first: cx.focus_handle().tab_index(1).tab_stop(true),
                second: cx.focus_handle().tab_index(2).tab_stop(true),
            }
        }
    }

    impl Render for FocusTraversalHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            FocusTraversal::new(
                div()
                    .size_full()
                    .child(
                        div()
                            .id("focus-traversal-first")
                            .track_focus(&self.first)
                            .size(px(1.0)),
                    )
                    .child(
                        div()
                            .id("focus-traversal-second")
                            .track_focus(&self.second)
                            .size(px(1.0)),
                    ),
            )
        }
    }

    #[gpui::test]
    fn focus_traversal_default_context_handles_tab(cx: &mut TestAppContext) {
        cx.update(init);
        let (harness, cx) = cx.add_window_view(|_, cx| FocusTraversalHarness::new(cx));
        let (first, second) = harness.read_with(cx, |harness, _| {
            (harness.first.clone(), harness.second.clone())
        });

        cx.update(|window, cx| {
            window.focus(&first, cx);
            window.dispatch_keystroke(gpui::Keystroke::parse("tab").unwrap(), cx);
            assert!(second.is_focused(window));
        });
    }

    #[gpui::test]
    fn focus_traversal_default_context_handles_shift_tab(cx: &mut TestAppContext) {
        cx.update(init);
        let (harness, cx) = cx.add_window_view(|_, cx| FocusTraversalHarness::new(cx));
        let (first, second) = harness.read_with(cx, |harness, _| {
            (harness.first.clone(), harness.second.clone())
        });

        cx.update(|window, cx| {
            window.focus(&second, cx);
            window.dispatch_keystroke(gpui::Keystroke::parse("shift-tab").unwrap(), cx);
            assert!(first.is_focused(window));
        });
    }

    #[gpui::test]
    fn focus_traversal_handles_next_and_previous(cx: &mut TestAppContext) {
        let (harness, cx) = cx.add_window_view(|_, cx| FocusTraversalHarness::new(cx));
        let (first, second) = harness.read_with(cx, |harness, _| {
            (harness.first.clone(), harness.second.clone())
        });

        cx.update(|window, cx| {
            window.focus(&first, cx);
            first.dispatch_action(&FocusNext, window, cx);
            assert!(second.is_focused(window));
            second.dispatch_action(&FocusPrevious, window, cx);
            assert!(first.is_focused(window));
        });
    }
}
