// ABOUTME: Role-based focus registry for major application surfaces
// ABOUTME: Keeps focus restoration separate from legacy global input dispatch

use std::sync::{Arc, RwLock};

use gpui::{App, FocusHandle, Window};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_coordinator_clear_methods_are_idempotent() {
        let coordinator = FocusCoordinator::default();

        coordinator.clear_picker_focus();
        coordinator.clear_prompt_focus();
        coordinator.clear_completion_focus();

        assert!(coordinator.picker_focus().is_none());
        assert!(coordinator.prompt_focus().is_none());
        assert!(coordinator.completion_focus().is_none());
    }
}
