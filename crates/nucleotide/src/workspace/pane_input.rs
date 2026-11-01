//! Route retained macro keys through the same native UI that physical keys use.
use super::*;

impl Workspace {
    pub(super) fn replay_native_ui_key(&mut self, cx: &mut Context<Self>) {
        if self.replay_ui_scheduled {
            return;
        }
        self.replay_ui_scheduled = true;
        let workspace = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = cx.with_window(workspace.entity_id(), |window, _| {
                window.on_next_frame(move |window, cx| {
                    let key = workspace
                        .update(cx, |workspace, cx| {
                            workspace.replay_ui_scheduled = false;
                            let overlay = workspace.overlay.read(cx);
                            if !overlay.has_prompt() && !overlay.has_picker() {
                                return None;
                            }
                            workspace
                                .core
                                .update(cx, |core, _| core.editor_input.take_suspended_replay_key())
                        })
                        .ok()
                        .flatten();
                    let Some(key) = key else {
                        return;
                    };
                    let (name, character) = match key.code {
                        KeyCode::Char(character) => {
                            (character.to_string(), Some(character.to_string()))
                        }
                        KeyCode::Enter => ("enter".into(), None),
                        KeyCode::Esc => ("escape".into(), None),
                        KeyCode::Tab => ("tab".into(), None),
                        KeyCode::Backspace => ("backspace".into(), None),
                        KeyCode::Delete => ("delete".into(), None),
                        KeyCode::Left => ("left".into(), None),
                        KeyCode::Right => ("right".into(), None),
                        KeyCode::Up => ("up".into(), None),
                        KeyCode::Down => ("down".into(), None),
                        KeyCode::Home => ("home".into(), None),
                        KeyCode::End => ("end".into(), None),
                        KeyCode::PageUp => ("pageup".into(), None),
                        KeyCode::PageDown => ("pagedown".into(), None),
                        KeyCode::F(number) => (format!("f{number}"), None),
                        _ => {
                            let _ = workspace.update(cx, |workspace, cx| {
                                workspace.core.update(cx, |core, _| {
                                    core.editor_input
                                        .cancel_workspace_commands(&mut core.editor);
                                    core.editor.set_error("Unsupported native UI macro key");
                                })
                            });
                            return;
                        }
                    };
                    window.dispatch_keystroke(
                        gpui::Keystroke {
                            key: name,
                            key_char: character,
                            modifiers: gpui::Modifiers {
                                control: key.modifiers.contains(KeyModifiers::CONTROL),
                                alt: key.modifiers.contains(KeyModifiers::ALT),
                                shift: key.modifiers.contains(KeyModifiers::SHIFT),
                                platform: key.modifiers.contains(KeyModifiers::SUPER),
                                ..Default::default()
                            },
                        },
                        cx,
                    );
                    let _ =
                        workspace.update(cx, |workspace, cx| workspace.replay_native_ui_key(cx));
                });
            });
        });
    }
}
