// ABOUTME: Workspace-owned terminal descriptors, stable views, and owning-surface sizing.
// ABOUTME: Placement is owned by the pane layout; moving a session never launches a PTY.

use std::collections::HashMap;

use gpui::{App, AppContext, Entity, FocusHandle};
use nucleotide_events::{
    run::RunId,
    terminal::{Event as TerminalEvent, TerminalId},
};
use nucleotide_terminal::{TerminalBounds, session::TerminalSessionCfg};
use nucleotide_terminal_view::TerminalView;

use crate::application::TerminalRuntimeHandle;

/// Integration supplies an opaque owning-surface token (pane or dock).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalHost(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalSessionState {
    Spawning,
    Running,
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
    },
    Failed(String),
    Closing,
}

pub struct TerminalSessionDescriptor {
    pub id: TerminalId,
    pub generation: u64,
    /// The user's launch intent. Remote/proxy/environment resolution stays in Workspace.
    /// Split callers clone this spec but allocate a fresh ID and call open again.
    pub spec: TerminalSessionCfg,
    pub run_id: Option<RunId>,
    pub state: TerminalSessionState,
    pub view: Entity<TerminalView>,
    pub focus: FocusHandle,
    pub host: Option<TerminalHost>,
    pub visible: bool,
    pub last_bounds: Option<TerminalBounds>,
}

pub struct TerminalSessions {
    generation: u64,
    sessions: HashMap<TerminalId, TerminalSessionDescriptor>,
}

impl TerminalSessions {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            sessions: HashMap::new(),
        }
    }

    pub fn get(&self, id: TerminalId) -> Option<&TerminalSessionDescriptor> {
        self.sessions.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &TerminalSessionDescriptor> {
        self.sessions.values()
    }

    /// Reserve before starting asynchronous environment/proxy resolution. Does not launch.
    /// Repeated opens and tombstoned IDs cannot replace an existing view/process.
    pub fn open(
        &mut self,
        id: TerminalId,
        spec: TerminalSessionCfg,
        run_id: Option<RunId>,
        runtime: &TerminalRuntimeHandle,
        cx: &mut App,
    ) -> bool {
        if self.sessions.contains_key(&id) {
            return false;
        }
        let Some(model) = runtime.prepare(id) else {
            return false;
        };
        let focus = cx.focus_handle();
        let view = cx.new(|cx| TerminalView::new(model, focus.clone(), cx));
        self.sessions.insert(
            id,
            TerminalSessionDescriptor {
                id,
                generation: self.generation,
                spec,
                run_id,
                state: TerminalSessionState::Spawning,
                view,
                focus,
                host: None,
                visible: false,
                last_bounds: None,
            },
        );
        true
    }

    /// Change only presentation ownership; keep the entity, selection, scrollback and focus.
    /// Hidden dock sessions still have one placement and retain their last nonzero size.
    pub fn set_host(&mut self, id: TerminalId, host: TerminalHost, visible: bool) -> bool {
        let Some(session) = self.sessions.get_mut(&id) else {
            return false;
        };
        session.host = Some(host);
        session.visible = visible;
        true
    }

    /// Only the current visible owner may resize. Stale pre-move layout callbacks are ignored.
    pub fn resize(
        &mut self,
        id: TerminalId,
        host: TerminalHost,
        bounds: TerminalBounds,
        runtime: &TerminalRuntimeHandle,
    ) -> bool {
        let Some(session) = self.sessions.get_mut(&id) else {
            return false;
        };
        if session.host != Some(host)
            || !session.visible
            || bounds.cols() == 0
            || bounds.rows() == 0
            || session
                .last_bounds
                .is_some_and(|previous| previous.approx_eq(&bounds))
        {
            return false;
        }
        session.last_bounds = Some(bounds);
        let (cell_width, cell_height) = bounds.cell_size();
        runtime.dispatch(&TerminalEvent::Resized {
            id,
            cols: bounds.cols(),
            rows: bounds.rows(),
            cell_width,
            cell_height,
        });
        true
    }

    /// Called only by the typed application update route, never from a worker.
    /// A true result means integration should refresh UI and complete the associated run,
    /// even if this terminal is not the active tab. Duplicate/late facts do nothing.
    pub fn apply_event(&mut self, generation: u64, event: &TerminalEvent) -> bool {
        if generation != self.generation {
            return false;
        }
        let (id, state) = match event {
            TerminalEvent::Spawned { id } => (*id, TerminalSessionState::Running),
            TerminalEvent::SpawnFailed { id, error } => {
                (*id, TerminalSessionState::Failed(error.clone()))
            }
            TerminalEvent::Exited { id, code, signal } => (
                *id,
                TerminalSessionState::Exited {
                    code: *code,
                    signal: *signal,
                },
            ),
            _ => return false,
        };
        let Some(session) = self.sessions.get_mut(&id) else {
            return false;
        };
        match (&session.state, &state) {
            (TerminalSessionState::Spawning, _) => {}
            (TerminalSessionState::Running, TerminalSessionState::Exited { .. }) => {}
            _ => return false,
        }
        session.state = state;
        true
    }

    /// Explicit close removes presentation and cancels both live and not-yet-dispatched launches.
    /// Return the association so integration can cancel that run independently of the active tab.
    /// Only unfinished sessions become Closing; closing retained output must not cancel a finished run.
    pub fn close(
        &mut self,
        id: TerminalId,
        runtime: &TerminalRuntimeHandle,
    ) -> Option<TerminalSessionDescriptor> {
        let mut session = self.sessions.remove(&id)?;
        if matches!(
            session.state,
            TerminalSessionState::Spawning | TerminalSessionState::Running
        ) {
            session.state = TerminalSessionState::Closing;
        }
        runtime.dispatch(&TerminalEvent::TerminateRequested { id });
        Some(session)
    }

    pub fn reset(&mut self, generation: u64, runtime: &TerminalRuntimeHandle) {
        for id in self.sessions.keys() {
            runtime.dispatch(&TerminalEvent::TerminateRequested { id: *id });
        }
        self.sessions.clear();
        self.generation = generation;
    }
}

#[cfg(test)]
#[path = "terminal_sessions_tests.rs"]
mod tests;
