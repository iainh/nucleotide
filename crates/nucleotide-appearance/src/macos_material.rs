//! Continuous native chrome material below GPUI's opaque editor surface.

use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained, runtime::AnyClass};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSGlassEffectView, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Owns one continuous native material behind the window's GPUI view.
/// Like GPUI windows, this must be updated and dropped on the main thread.
#[derive(Default)]
pub struct ChromeMaterial {
    view: Option<Retained<NSView>>,
    is_dark: Option<bool>,
}

impl ChromeMaterial {
    /// Cover the GPUI frame when enabled, or hide the material for themed chrome.
    /// Returns whether native material is visible. All content stays in GPUI;
    /// the editor island paints its own opaque surface above this backdrop.
    pub fn sync(
        &mut self,
        window: &gpui::Window,
        enabled: bool,
        is_dark: bool,
    ) -> Result<bool, String> {
        if !enabled {
            if let Some(view) = &self.view {
                view.setHidden(true);
            }
            return Ok(false);
        }

        let mtm = MainThreadMarker::new().ok_or("Chrome material requires the main thread")?;
        let handle = HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err("Window does not have an AppKit handle".into());
        };
        // SAFETY: GPUI lends its live NSView, and the main-thread check above
        // ensures all AppKit access stays on the window's UI thread.
        let gpui_view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        // SAFETY: The live window owns this hierarchy; retain the parent before
        // doing any AppKit work that could change its subviews.
        let parent = unsafe { gpui_view.superview() }.ok_or("GPUI view has no superview")?;
        let frame = gpui_view.frame();

        let view = self.view.get_or_insert_with(|| {
            // Query the runtime before referencing this macOS 26+ class. Older
            // systems use the semantic sidebar material, not a simulated glass.
            if AnyClass::get(c"NSGlassEffectView").is_some() {
                let glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), frame);
                glass.setCornerRadius(0.0);
                glass.into_super()
            } else {
                let effect =
                    NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
                effect.setMaterial(NSVisualEffectMaterial::Sidebar);
                effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
                effect.setState(NSVisualEffectState::FollowsWindowActiveState);
                effect.into_super()
            }
        });
        // Reattach if AppKit changes the hierarchy during fullscreen transitions.
        // SAFETY: Both views are retained throughout this synchronous update.
        if unsafe { view.superview() }.as_deref() != Some(parent.as_ref()) {
            parent.addSubview_positioned_relativeTo(
                view,
                NSWindowOrderingMode::Below,
                Some(gpui_view),
            );
        }
        if view.frame() != frame {
            view.setFrame(frame);
        }
        if self.is_dark != Some(is_dark) {
            // Set only the material's appearance. The window and GPUI view must
            // continue inheriting actual OS changes for theme observation.
            let name = unsafe {
                if is_dark {
                    NSAppearanceNameDarkAqua
                } else {
                    NSAppearanceNameAqua
                }
            };
            let appearance =
                NSAppearance::appearanceNamed(name).ok_or("AppKit appearance unavailable")?;
            view.setAppearance(Some(&appearance));
            self.is_dark = Some(is_dark);
        }
        view.setHidden(false);
        Ok(true)
    }
}

impl Drop for ChromeMaterial {
    fn drop(&mut self) {
        if let Some(view) = &self.view {
            // Removing before releasing prevents an orphaned material when a
            // workspace is replaced without destroying its native window.
            view.removeFromSuperview();
        }
    }
}
