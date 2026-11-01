//! Terminal placement, focus, and lifecycle on the same pane surfaces as documents.
#[cfg(feature = "terminal-emulator-core")]
use super::terminal_sessions::{TerminalHost, TerminalSessionState};
use super::*;
use nucleotide_terminal::session::TerminalSessionCfg;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TERMINAL: AtomicU64 = AtomicU64::new(1);
pub(super) fn next_terminal_id() -> TerminalId {
    TerminalId(NEXT_TERMINAL.fetch_add(1, Ordering::Relaxed))
}

impl Workspace {
    pub(super) fn duplicate_terminal(
        &mut self,
        id: TerminalId,
        cx: &mut Context<Self>,
    ) -> Result<TerminalId, String> {
        #[cfg(feature = "terminal-emulator-core")]
        {
            let spec = self
                .terminal_sessions
                .get(id)
                .ok_or("Terminal no longer exists")?
                .spec
                .clone();
            Ok(if let Some(program) = spec.program {
                self.spawn_terminal_command_session(spec.cwd, program, spec.args, spec.env, cx)
            } else {
                self.spawn_terminal_session_with_input(spec.cwd, spec.env, None, cx)
            })
        }
        #[cfg(not(feature = "terminal-emulator-core"))]
        {
            let _ = (id, cx);
            Err("Terminal emulator is disabled".into())
        }
    }

    pub(super) fn activate_terminal_tab(
        &mut self,
        pane: PaneId,
        id: TerminalId,
        cx: &mut Context<Self>,
    ) {
        if self
            .pane_host
            .layout
            .activate_tab(pane, TabId::Terminal(id))
            .is_err()
        {
            return;
        }
        self.project_panes(cx);
        self.hide_completions(cx);
        self.defer_focus_editor(cx);
        cx.notify();
    }

    pub(super) fn reserve_terminal(
        &mut self,
        id: TerminalId,
        spec: TerminalSessionCfg,
        run: Option<RunId>,
        cx: &mut Context<Self>,
    ) {
        #[cfg(feature = "terminal-emulator-core")]
        {
            let runtime = self.core.read(cx).terminal_runtime.clone();
            self.terminal_sessions.open(id, spec, run, &runtime, cx);
        }
        #[cfg(not(feature = "terminal-emulator-core"))]
        let _ = (id, spec, run, cx);
    }

    pub(super) fn mount_terminal(&mut self, id: TerminalId, pane: PaneId, cx: &mut Context<Self>) {
        let _ = self
            .pane_host
            .layout
            .open_tab(pane, TabId::Terminal(id), true);
        self.project_panes(cx);
        self.hide_completions(cx);
        self.update_document_views(cx);
        self.defer_focus_editor(cx);
        self.invalidate_tab_bar_documents();
        cx.notify();
    }

    pub(super) fn mount_dock_terminal(&mut self, id: TerminalId, cx: &mut Context<Self>) {
        let pane = self
            .pane_host
            .layout
            .ensure_dock(self.basic_terminal_height)
            .unwrap();
        self.pane_host.layout.set_dock_visible(true);
        self.terminal_panel_visible = true;
        self.last_dock_pane = Some(pane);
        self.mount_terminal(id, pane, cx);
    }

    pub(super) fn sync_terminal_hosts(&mut self) {
        #[cfg(feature = "terminal-emulator-core")]
        {
            let visible: HashSet<_> = self
                .pane_host
                .layout
                .layout(
                    PaneRect {
                        x: 0.0,
                        y: 0.0,
                        width: 1000.0,
                        height: 1000.0,
                    },
                    PaneSize::default(),
                )
                .unwrap_or_default()
                .into_iter()
                .map(|geometry| geometry.pane)
                .collect();
            for pane in self.pane_host.layout.panes() {
                let state = self.pane_host.layout.pane(pane).unwrap();
                for tab in state.tabs() {
                    if let TabId::Terminal(id) = tab {
                        self.terminal_sessions.set_host(
                            *id,
                            TerminalHost(pane.key()),
                            visible.contains(&pane) && state.active_tab() == Some(*tab),
                        );
                    }
                }
            }
        }
    }

    pub(super) fn focused_terminal_id(&self, window: &Window) -> Option<TerminalId> {
        #[cfg(feature = "terminal-emulator-core")]
        {
            self.terminal_sessions
                .iter()
                .find_map(|session| session.focus.is_focused(window).then_some(session.id))
        }
        #[cfg(not(feature = "terminal-emulator-core"))]
        {
            let _ = window;
            None
        }
    }

    pub(super) fn active_terminal_focus(&self) -> Option<FocusHandle> {
        #[cfg(feature = "terminal-emulator-core")]
        {
            let TabId::Terminal(id) = self
                .pane_host
                .layout
                .pane(self.focused_pane_id())?
                .active_tab()?
            else {
                return None;
            };
            self.terminal_sessions
                .get(id)
                .map(|session| session.focus.clone())
        }
        #[cfg(not(feature = "terminal-emulator-core"))]
        {
            None
        }
    }

    pub(super) fn render_terminal_content(
        &mut self,
        pane: PaneId,
        id: TerminalId,
        bounds: PaneRect,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        #[cfg(feature = "terminal-emulator-core")]
        {
            let (line, cell, _, _, _) = self.get_focused_document_view_layout(cx);
            let height = bounds.height - f32::from(self.pane_tab_bar_height(pane, cx));
            if bounds.width > 0.0 && height > 0.0 {
                let metrics = TerminalBounds::from_pixels(
                    f32::from(cell).max(1.0),
                    f32::from(line).max(1.0),
                    bounds.width,
                    height,
                );
                let runtime = self.core.read(cx).terminal_runtime.clone();
                self.terminal_sessions
                    .resize(id, TerminalHost(pane.key()), metrics, &runtime);
            }
            if let Some(session) = self.terminal_sessions.get(id) {
                let view = session.view.clone();
                return div()
                    .size_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |workspace, _, window, cx| {
                            workspace.focus_tab_pane(pane, cx);
                            workspace.focus_editor(window, cx);
                        }),
                    )
                    .child(view)
                    .into_any_element();
            }
        }
        let _ = (pane, id, bounds, cx);
        div().child("Terminal unavailable").into_any_element()
    }

    pub(super) fn handle_terminal_event(&mut self, event: &TerminalEvent, cx: &mut Context<Self>) {
        #[cfg(feature = "terminal-emulator-core")]
        {
            if !self
                .terminal_sessions
                .apply_event(self.terminal_generation, event)
            {
                return;
            }
            let (id, message, severity) = match event {
                TerminalEvent::Exited { id, code, signal } => (
                    *id,
                    format!(
                        "Terminal {} exited{}{}",
                        id.0,
                        code.map(|code| format!(" ({code})")).unwrap_or_default(),
                        signal
                            .map(|signal| format!(" signal {signal}"))
                            .unwrap_or_default()
                    ),
                    if code == &Some(0) {
                        Severity::Info
                    } else {
                        Severity::Warning
                    },
                ),
                TerminalEvent::SpawnFailed { id, error } => (
                    *id,
                    format!("Terminal {} failed: {error}", id.0),
                    Severity::Error,
                ),
                TerminalEvent::Spawned { .. } => {
                    cx.notify();
                    return;
                }
                _ => return,
            };
            if let Some((_run, activity)) = self.terminal_activities.remove(&id) {
                self.finish_background_activity(activity, cx);
                self.push_editor_status_notification(
                    EditorStatus {
                        status: message,
                        severity,
                    },
                    cx,
                );
            }
            cx.notify();
        }
        #[cfg(not(feature = "terminal-emulator-core"))]
        let _ = (event, cx);
    }

    pub(super) fn close_terminal_tab(
        &mut self,
        pane: PaneId,
        id: TerminalId,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        #[cfg(feature = "terminal-emulator-core")]
        {
            if !force
                && self.terminal_sessions.get(id).is_some_and(|session| {
                    session.run_id.is_some()
                        && matches!(
                            session.state,
                            TerminalSessionState::Running | TerminalSessionState::Spawning
                        )
                })
            {
                self.terminal_close_confirm = Some((pane, id));
                self.defer_confirmation_modal(cx);
                return;
            }
            let runtime = self.core.read(cx).terminal_runtime.clone();
            self.terminal_sessions.close(id, &runtime);
            if let Some((_run, activity)) = self.terminal_activities.remove(&id) {
                self.finish_background_activity(activity, cx);
            }
        }
        let _ = force;
        let _ = self.pane_host.layout.close_tab(pane, TabId::Terminal(id));
        let layout = &self.pane_host.layout;
        let main = layout.main_panes();
        if layout.dock().is_some_and(|dock| dock.visible())
            && layout
                .panes()
                .into_iter()
                .filter(|id| !main.contains(id))
                .all(|id| layout.pane(id).unwrap().tabs().is_empty())
        {
            self.hide_terminal_panel(cx);
        } else {
            self.project_panes(cx);
            self.update_document_views(cx);
        }
        self.invalidate_tab_bar_documents();
        cx.notify();
    }

    pub(super) fn handle_terminal_close_confirmation(
        &mut self,
        event: ConfirmDialogEvent,
        cx: &mut Context<Self>,
    ) {
        if let Some((pane, id)) = self.terminal_close_confirm.take() {
            if matches!(event, ConfirmDialogEvent::Confirmed) {
                self.close_terminal_tab(pane, id, true, cx);
            }
        }
        self.defer_focus_editor(cx);
    }
}
