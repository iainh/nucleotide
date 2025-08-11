// ABOUTME: Provides editor capabilities through dependency injection
// ABOUTME: Allows components to access editor functionality without direct Core dependency

use gpui::{Global, WeakEntity};
use nucleotide_core::{EditorReadAccess, EditorWriteAccess};

/// Global provider for editor capabilities
/// This is stored in GPUI's global context for dependency injection
pub struct EditorProvider {
    core: WeakEntity<crate::Core>,
}

impl EditorProvider {
    pub fn new(core: WeakEntity<crate::Core>) -> Self {
        Self { core }
    }

    pub fn with_core<R>(&self, cx: &gpui::App, f: impl FnOnce(&crate::Core) -> R) -> Option<R> {
        self.core.upgrade().map(|core| f(core.read(cx)))
    }

    pub fn with_core_mut<R>(
        &self,
        cx: &mut gpui::App,
        f: impl FnOnce(&mut crate::Core) -> R,
    ) -> Option<R> {
        self.core
            .upgrade()
            .map(|core| core.update(cx, |core, _| f(core)))
    }
}

impl Global for EditorProvider {}

/// Extension trait for accessing editor through the global provider
pub trait EditorProviderExt {
    fn with_editor<R>(&self, f: impl FnOnce(&helix_view::Editor) -> R) -> Option<R>;
    fn with_editor_mut<R>(&mut self, f: impl FnOnce(&mut helix_view::Editor) -> R) -> Option<R>;
}

impl EditorProviderExt for gpui::App {
    fn with_editor<R>(&self, f: impl FnOnce(&helix_view::Editor) -> R) -> Option<R> {
        self.try_global::<EditorProvider>()
            .and_then(|provider| provider.with_core(self, |core| f(core.editor())))
    }

    fn with_editor_mut<R>(&mut self, f: impl FnOnce(&mut helix_view::Editor) -> R) -> Option<R> {
        // We need to clone the weak reference to avoid borrowing issues
        let core_weak = self.try_global::<EditorProvider>()?.core.clone();
        core_weak
            .upgrade()
            .map(|core| core.update(self, |core, _| f(core.editor_mut())))
    }
}
