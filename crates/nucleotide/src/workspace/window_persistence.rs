//! Saves this window's geometry and chrome state, and opens new windows.

use std::time::Duration;

use gpui::{AppContext as _, Context, Subscription, Task, Window};
use nucleotide_logging::{error, info};

use super::{DOC_SIDEBAR_DEFAULT_WIDTH, Workspace};
use crate::window_state::{self, ChromeState, SavedWindow, WindowState};

/// Window moves and drag-resizes report many intermediate bounds; save once they settle.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);

pub(super) struct WindowStatePersistence {
    /// The state last written or scheduled to be written.
    saved: WindowState,
    pending_save: Option<Task<()>>,
    _subscriptions: [Subscription; 3],
}

impl Workspace {
    /// Restores the saved chrome state and keeps the state file current from now on.
    ///
    /// Only the application window calls this, so tests and other workspaces never touch the
    /// user's state file.
    pub fn enable_window_state_persistence(
        &mut self,
        state: WindowState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_chrome_state(state.chrome);
        let subscriptions = [
            cx.observe_window_bounds(window, |workspace, window, cx| {
                workspace.persist_window_state(window, cx);
            }),
            cx.on_app_quit(|workspace, _cx| {
                workspace.flush_window_state();
                async {}
            }),
            // Closing the window drops the workspace, which would cancel a pending save.
            cx.on_release(|workspace, _cx| workspace.flush_window_state()),
        ];
        self.window_state_persistence = Some(WindowStatePersistence {
            saved: state,
            pending_save: None,
            _subscriptions: subscriptions,
        });
    }

    fn apply_chrome_state(&mut self, chrome: ChromeState) {
        self.show_file_tree = chrome.show_file_tree;
        // Render clamps both widths to the viewport before they are used.
        if let Some(width) = chrome.file_tree_width {
            self.file_tree_width = width;
            self.file_tree_width_override = Some(width);
        }
        self.doc_sidebar_width = chrome
            .doc_sidebar_width
            .unwrap_or(DOC_SIDEBAR_DEFAULT_WIDTH);
    }

    fn current_chrome_state(&self) -> ChromeState {
        ChromeState {
            show_file_tree: self.show_file_tree,
            file_tree_width: self.file_tree_width_override,
            doc_sidebar_width: ((self.doc_sidebar_width - DOC_SIDEBAR_DEFAULT_WIDTH).abs() > 0.5)
                .then_some(self.doc_sidebar_width),
        }
    }

    fn current_window_state(&self, window: &Window, cx: &Context<Self>) -> WindowState {
        let display_uuid = window
            .display(cx)
            .and_then(|display| display.uuid().ok())
            .map(|uuid| uuid.to_string());
        WindowState {
            window: Some(SavedWindow::from_gpui(window.window_bounds(), display_uuid)),
            chrome: self.current_chrome_state(),
        }
    }

    /// Called on every render: compares only the chrome, which is cheap, and saves when it
    /// changed. Window bounds changes arrive through the bounds observer instead.
    pub(super) fn persist_chrome_state(&mut self, window: &Window, cx: &mut Context<Self>) {
        let changed = self
            .window_state_persistence
            .as_ref()
            .is_some_and(|persistence| persistence.saved.chrome != self.current_chrome_state());
        if changed {
            self.persist_window_state(window, cx);
        }
    }

    /// Schedules a save when the window or its chrome changed since the last save.
    fn persist_window_state(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.window_state_persistence.is_none() {
            return;
        }
        let state = self.current_window_state(window, cx);
        let Some(persistence) = self.window_state_persistence.as_mut() else {
            return;
        };
        if persistence.saved == state {
            return;
        }

        persistence.saved = state.clone();
        // Replacing the task cancels the previous save, so only the settled state is written.
        persistence.pending_save = Some(cx.spawn(async move |workspace, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            cx.background_spawn(async move { window_state::save(&state) })
                .await;
            let _ = workspace.update(cx, |workspace, _| {
                if let Some(persistence) = workspace.window_state_persistence.as_mut() {
                    persistence.pending_save = None;
                }
            });
        }));
    }

    /// Writes a pending save now instead of after the debounce.
    fn flush_window_state(&mut self) {
        let Some(persistence) = self.window_state_persistence.as_mut() else {
            return;
        };
        if persistence.pending_save.take().is_some() {
            window_state::save(&persistence.saved);
        }
    }

    pub(super) fn open_new_window(&mut self, cx: &mut Context<Self>) {
        // The new window cascades from the saved bounds; write any save still being debounced.
        self.flush_window_state();

        match window_state::spawn_new_window_process() {
            Ok(()) => info!("Opened a new Nucleotide window"),
            Err(error) => {
                error!(error = %error, "Failed to open a new window");
                self.core.update(cx, |core, cx| {
                    core.editor
                        .set_error(format!("Failed to open a new window: {error}"));
                    cx.notify();
                });
            }
        }
    }
}
