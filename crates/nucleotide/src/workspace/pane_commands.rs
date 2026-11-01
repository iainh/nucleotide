//! Workspace operations yielded by Helix's ordered command interpreter.
use super::*;
use crate::application::editor_input::{
    EditorCommandToken, EditorContinuation, NativeFileOpenAction,
    workspace_commands::{
        WorkspaceCommand, WorkspaceCommandRequest, WorkspaceDirection, validate_workspace_quit,
    },
};

impl Workspace {
    pub(super) fn begin_command_completion(
        &mut self,
        token: EditorCommandToken,
        cx: &mut Context<Self>,
    ) -> bool {
        self.core.update(cx, |core, _| {
            core.editor_input
                .begin_command_completion(token, &mut core.editor)
        })
    }

    pub(super) fn resume_command_continuation(
        &mut self,
        token: EditorCommandToken,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_effects(cx);
        self.project_panes(cx);
        let handle = self.handle.clone();
        self.core.update(cx, |core, cx| {
            core.handle_input_event(InputEvent::ResumeCommand(token), cx, handle)
        });
    }

    pub(super) fn handoff_command_ui(
        &mut self,
        continuation: Option<EditorContinuation>,
        cx: &mut Context<Self>,
    ) {
        let slot = self.pane_host.active_slot();
        self.ui_continuation = continuation.and_then(|continuation| {
            self.core.update(cx, |core, _| {
                core.editor_input
                    .acknowledge_command_ui(continuation.token, slot, &core.editor)
            })
        });
        let continuation = self.ui_continuation;
        self.core.update(cx, |_, cx| {
            cx.emit(crate::Update::NativeUiContinuation(continuation));
        });
    }

    pub(super) fn handle_workspace_command(
        &mut self,
        request: &WorkspaceCommandRequest,
        continuation: Option<EditorContinuation>,
        cx: &mut Context<Self>,
    ) {
        if let WorkspaceCommand::Open { paths, action } = &request.command {
            self.open_command_paths(paths.clone(), *action, continuation, cx);
            return;
        }
        if let Some(continuation) = continuation
            && !self.begin_command_completion(continuation.token, cx)
        {
            return;
        }
        self.apply_editor_effects(cx);
        let result = (|| -> Result<bool, String> {
            // Helix's pane commands ignore a numeric prefix. The interpreter
            // retains that prefix for subsequent editing commands and macros.
            {
                let pane = self.focused_pane_id();
                match &request.command {
                    WorkspaceCommand::Reconcile => {}
                    WorkspaceCommand::Focus(direction) | WorkspaceCommand::Swap(direction) => {
                        let direction = *direction;
                        let panes: Vec<_> = self
                            .pane_host
                            .layout
                            .layout(self.pane_bounds(), PaneSize::default())
                            .map_err(|error| format!("{error:?}"))?
                            .into_iter()
                            .map(|geometry| geometry.pane)
                            .collect();
                        let index = panes.iter().position(|id| *id == pane).unwrap();
                        let target = match direction {
                            WorkspaceDirection::Next => Some(panes[(index + 1) % panes.len()]),
                            WorkspaceDirection::Previous => {
                                Some(panes[(index + panes.len() - 1) % panes.len()])
                            }
                            direction => {
                                let edge = match direction {
                                    WorkspaceDirection::Left => pane_layout::Edge::Left,
                                    WorkspaceDirection::Right => pane_layout::Edge::Right,
                                    WorkspaceDirection::Up => pane_layout::Edge::Top,
                                    WorkspaceDirection::Down => pane_layout::Edge::Bottom,
                                    _ => unreachable!(),
                                };
                                self.pane_host
                                    .layout
                                    .neighbor(pane, edge, self.pane_bounds(), PaneSize::default())
                                    .map_err(|error| format!("{error:?}"))?
                            }
                        };
                        if let Some(target) = target {
                            if matches!(request.command, WorkspaceCommand::Swap(_)) {
                                self.pane_host
                                    .layout
                                    .swap_panes(pane, target)
                                    .map_err(|error| format!("{error:?}"))?;
                            } else {
                                self.pane_host
                                    .layout
                                    .activate_pane(target)
                                    .map_err(|error| format!("{error:?}"))?;
                            }
                        }
                    }
                    WorkspaceCommand::Transpose => {
                        self.pane_host
                            .layout
                            .transpose(pane_layout::Node::Pane(pane))
                            .map_err(|error| format!("{error:?}"))?;
                    }
                    WorkspaceCommand::Split {
                        action,
                        new_document,
                    } => {
                        let active = self.pane_host.layout.pane(pane).unwrap().active_tab();
                        let tab = if *new_document || active.is_none() {
                            let handle = self.handle.clone();
                            let document = self.core.update(cx, |core, _| {
                                let _guard = handle.enter();
                                let document =
                                    core.editor.new_file(helix_view::editor::Action::Load);
                                // Host owns this placement. Suppress registration echo only.
                                let _ = core.editor.drain_hosted_effects();
                                document
                            });
                            TabId::Document(document)
                        } else if let Some(TabId::Terminal(id)) = active {
                            TabId::Terminal(self.duplicate_terminal(id, cx)?)
                        } else {
                            active.unwrap()
                        };
                        let edge = match action {
                            NativeFileOpenAction::HorizontalSplit => pane_layout::Edge::Bottom,
                            _ => pane_layout::Edge::Right,
                        };
                        let destination = self
                            .pane_host
                            .layout
                            .split_pane(pane, edge, pane_layout::SplitMode::Sibling, vec![tab])
                            .map_err(|error| format!("{error:?}"))?;
                        self.pane_host.transfer_state(pane, destination);
                    }
                    WorkspaceCommand::ClosePane { force, quit } => {
                        // An allocated but empty hidden dock is not a live
                        // workspace region and must not swallow final :q.
                        let layout = &self.pane_host.layout;
                        let main = layout.main_panes();
                        let final_pane = main == [pane]
                            && layout.panes().into_iter().all(|id| {
                                id == pane
                                    || (!layout.dock().is_some_and(|dock| dock.visible())
                                        && layout.pane(id).unwrap().tabs().is_empty())
                            });
                        if final_pane && *quit {
                            validate_workspace_quit(&self.core.read(cx).editor, *force)?;
                            cx.emit(crate::Update::ShouldQuit);
                            return Ok(false);
                        }
                        self.pane_host
                            .layout
                            .close_pane(pane, None)
                            .map_err(|error| format!("{error:?}"))?;
                    }
                    WorkspaceCommand::KeepOnly => self
                        .pane_host
                        .layout
                        .keep_only(pane)
                        .map_err(|error| format!("{error:?}"))?,
                    WorkspaceCommand::QuitWorkspace { force } => {
                        validate_workspace_quit(&self.core.read(cx).editor, *force)?;
                        cx.emit(crate::Update::ShouldQuit);
                        return Ok(false);
                    }
                    WorkspaceCommand::Open { .. } => unreachable!(),
                }
            }
            Ok(true)
        })();
        match result {
            Ok(true) => {
                self.project_panes(cx);
                self.update_document_views(cx);
                self.defer_focus_editor(cx);
                if let Some(continuation) = continuation {
                    self.resume_command_continuation(continuation.token, cx);
                }
            }
            Ok(false) => self.core.update(cx, |core, _| {
                core.editor_input
                    .cancel_workspace_commands(&mut core.editor)
            }),
            Err(error) => self.core.update(cx, |core, _| {
                core.editor_input
                    .cancel_workspace_commands(&mut core.editor);
                core.editor.set_error(error);
            }),
        }
        cx.notify();
    }

    fn pane_bounds(&self) -> PaneRect {
        self.pane_bounds
    }

    pub(super) fn open_command_paths(
        &mut self,
        paths: Vec<String>,
        action: NativeFileOpenAction,
        continuation: Option<EditorContinuation>,
        cx: &mut Context<Self>,
    ) {
        let pane = self.focused_pane_id();
        let backend = self.core.read(cx).workspace_backend.clone();
        if matches!(backend.identity(), WorkspaceIdentity::Local) {
            if let Some(continuation) = continuation
                && !self.begin_command_completion(continuation.token, cx)
            {
                return;
            }
            for argument in paths {
                let (path, position) =
                    crate::application::editor_input::workspace_commands::parse_open_target(
                        &argument,
                    );
                if path.is_dir() {
                    self.workspace_picker_action = Some(action);
                    self.handoff_command_ui(continuation, cx);
                    self.core
                        .update(cx, |_, cx| cx.emit(crate::Update::ShowFilePickerAt(path)));
                    return;
                }
                if image_viewer::should_open_with_image_viewer(
                    &path,
                    position != Position::default(),
                    &WorkspaceIdentity::Local,
                ) {
                    let pane = match action {
                        NativeFileOpenAction::HorizontalSplit
                        | NativeFileOpenAction::VerticalSplit => {
                            let edge = if action == NativeFileOpenAction::HorizontalSplit {
                                pane_layout::Edge::Bottom
                            } else {
                                pane_layout::Edge::Right
                            };
                            self.pane_host
                                .layout
                                .split_pane(
                                    self.focused_pane_id(),
                                    edge,
                                    pane_layout::SplitMode::Sibling,
                                    vec![],
                                )
                                .unwrap()
                        }
                        _ => self.focused_pane_id(),
                    };
                    self.open_image_file_in_pane(pane, &path, &path, true, cx);
                    continue;
                }
                let handle = self.handle.clone();
                let result = self.core.update(cx, |core, _| {
                    let _guard = handle.enter();
                    core.editor.open(&path, action.into())
                });
                match result {
                    Ok(document) => {
                        self.apply_editor_effects(cx);
                        let slot = self.pane_host.active_slot();
                        self.core.update(cx, |core, _| {
                            if let Some(slot) = slot {
                                let view = core.editor.tree.get(slot);
                                let doc = core.editor.documents.get_mut(&document).unwrap();
                                let offset = pos_at_coords(doc.text().slice(..), position, true);
                                doc.set_selection(slot, Selection::point(offset));
                                helix_view::align_view(doc, view, helix_view::Align::Center);
                            }
                        });
                    }
                    Err(error) => {
                        self.core.update(cx, |core, _| {
                            core.editor_input
                                .cancel_workspace_commands(&mut core.editor);
                            core.editor.set_error(error.to_string());
                        });
                        return;
                    }
                }
            }
            self.update_document_views(cx);
            if let Some(continuation) = continuation {
                self.resume_command_continuation(continuation.token, cx);
            }
            return;
        }
        let generation = self.remote_open_generation;
        let handle = self.handle.clone();
        cx.spawn(async move |workspace, cx| {
            let reads = handle
                .spawn(async move {
                    let mut reads = Vec::new();
                    for argument in paths {
                        let (path, position) =
                            crate::application::editor_input::workspace_commands::parse_open_target(
                                &argument,
                            );
                        reads.push((
                            crate::application::native_open_file_with_backend(
                                backend.clone(),
                                path,
                            )
                            .await?,
                            position,
                        ));
                    }
                    Ok::<_, anyhow::Error>(reads)
                })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Some(continuation) = continuation
                    && !workspace.begin_command_completion(continuation.token, cx)
                {
                    return;
                }
                if workspace.remote_open_generation != generation
                    || workspace.pane_host.layout.pane(pane).is_none()
                {
                    workspace.core.update(cx, |core, _| {
                        core.editor_input
                            .cancel_workspace_commands(&mut core.editor)
                    });
                    return;
                }
                workspace.focus_tab_pane(pane, cx);
                let result = match reads {
                    Ok(result) => result,
                    Err(error) => Err(anyhow::anyhow!(error)),
                };
                match result {
                    Ok(reads) => {
                        if let Some(path) = reads.iter().find_map(|(read, _)| match read {
                            crate::application::NativeOpenFileOutcome::Directory(path) => {
                                Some(path.clone())
                            }
                            _ => None,
                        }) {
                            workspace.workspace_picker_action = Some(action);
                            workspace.handoff_command_ui(continuation, cx);
                            workspace
                                .core
                                .update(cx, |_, cx| cx.emit(crate::Update::ShowFilePickerAt(path)));
                            return;
                        }
                        let handle = workspace.handle.clone();
                        let result = workspace.core.update(cx, |core, cx| {
                            for (read, position) in reads {
                                match read {
                                    crate::application::NativeOpenFileOutcome::Document(read) => {
                                        let document =
                                            crate::application::open_workspace_document_from_read(
                                                &mut core.editor,
                                                read,
                                                action.into(),
                                            )?;
                                        let slot = core.editor.tree.focus;
                                        if let Some(doc) = core.editor.documents.get_mut(&document)
                                        {
                                            let offset =
                                                pos_at_coords(doc.text().slice(..), position, true);
                                            doc.set_selection(slot, Selection::point(offset));
                                        }
                                        core.attach_or_start_remote_document_servers(
                                            document, &handle, cx,
                                        );
                                    }
                                    crate::application::NativeOpenFileOutcome::Directory(_) => {
                                        anyhow::bail!("Use the file picker to open a directory")
                                    }
                                }
                            }
                            Ok::<_, anyhow::Error>(())
                        });
                        if let Err(error) = result {
                            workspace.core.update(cx, |core, _| {
                                core.editor_input
                                    .cancel_workspace_commands(&mut core.editor);
                                core.editor.set_error(error.to_string());
                            });
                        } else {
                            if let Some(continuation) = continuation {
                                workspace.resume_command_continuation(continuation.token, cx);
                            }
                        }
                    }
                    Err(error) => workspace.core.update(cx, |core, _| {
                        core.editor_input
                            .cancel_workspace_commands(&mut core.editor);
                        core.editor.set_error(error.to_string());
                    }),
                }
                workspace.update_document_views(cx);
                cx.notify();
            });
        })
        .detach();
    }
}
