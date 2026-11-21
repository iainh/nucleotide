// ABOUTME: Reusable popup menu primitives for application menus and menu-like controls
// ABOUTME: Adapts gpui-component menu patterns to Nucleotide's token system

use gpui::{App, KeyBinding};

use crate::actions::menu::{
    ActivateMenuBar, AltTapMenuBar, Cancel, Confirm, SelectDown, SelectLeft, SelectRight, SelectUp,
};

mod popup_menu;
mod popup_menu_surface;

pub use popup_menu::{MenuCheckSide, PopupMenu, PopupMenuItem};
pub use popup_menu_surface::PopupMenuSurface;

pub(crate) const POPUP_MENU_CONTEXT: &str = "PopupMenu";
pub(crate) const APP_MENU_BAR_CONTEXT: &str = "ApplicationMenu";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Confirm, Some(POPUP_MENU_CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(POPUP_MENU_CONTEXT)),
        KeyBinding::new("up", SelectUp, Some(POPUP_MENU_CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(POPUP_MENU_CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(POPUP_MENU_CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(POPUP_MENU_CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(APP_MENU_BAR_CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(APP_MENU_BAR_CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(APP_MENU_BAR_CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(APP_MENU_BAR_CONTEXT)),
        KeyBinding::new("space", Confirm, Some(APP_MENU_BAR_CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(APP_MENU_BAR_CONTEXT)),
    ]);

    // macOS uses the global menu bar. Elsewhere, bind only the bare `alt`
    // keystroke, which GPUI emits when Alt is pressed and released with no
    // other key, so Helix `Alt+<key>` bindings still reach the editor.
    if !cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("alt", AltTapMenuBar, None),
            KeyBinding::new("f10", ActivateMenuBar, None),
        ]);
    }
}
