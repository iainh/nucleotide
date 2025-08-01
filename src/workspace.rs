use std::collections::{HashMap, HashSet};

use gpui::prelude::FluentBuilder;
use gpui::*;
use helix_view::ViewId;
use log::info;

use crate::document::DocumentView;
use crate::info_box::InfoBoxView;
use crate::notification::NotificationView;
use crate::overlay::OverlayView;
use crate::utils;
use crate::{Core, Input, InputEvent};

pub struct Workspace {
    core: Model<Core>,
    input: Model<Input>,
    focused_view_id: Option<ViewId>,
    documents: HashMap<ViewId, View<DocumentView>>,
    handle: tokio::runtime::Handle,
    overlay: View<OverlayView>,
    info: View<InfoBoxView>,
    info_hidden: bool,
    notifications: View<NotificationView>,
}

impl Workspace {
    pub fn new(
        core: Model<Core>,
        input: Model<Input>,
        handle: tokio::runtime::Handle,
        cx: &mut ViewContext<Self>,
    ) -> Self {
        let notifications = Self::init_notifications(&core, cx);
        let info = Self::init_info_box(&core, cx);
        let overlay = cx.new_view(|cx| {
            let view = OverlayView::new(&cx.focus_handle());
            view.subscribe(&core, cx);
            view
        });

        Self {
            core,
            input,
            focused_view_id: None,
            handle,
            overlay,
            info,
            info_hidden: true,
            documents: HashMap::default(),
            notifications,
        }
    }

    fn init_notifications(
        editor: &Model<Core>,
        cx: &mut ViewContext<Self>,
    ) -> View<NotificationView> {
        let theme = Self::theme(&editor, cx);
        let text_style = theme.get("ui.text.info");
        let popup_style = theme.get("ui.popup.info");
        let popup_bg_color = utils::color_to_hsla(popup_style.bg.unwrap()).unwrap_or(black());
        let popup_text_color = utils::color_to_hsla(text_style.fg.unwrap()).unwrap_or(white());

        cx.new_view(|cx| {
            let view = NotificationView::new(popup_bg_color, popup_text_color);
            view.subscribe(&editor, cx);
            view
        })
    }

    fn init_info_box(editor: &Model<Core>, cx: &mut ViewContext<Self>) -> View<InfoBoxView> {
        let theme = Self::theme(editor, cx);
        let text_style = theme.get("ui.text.info");
        let popup_style = theme.get("ui.popup.info");
        let fg = text_style
            .fg
            .and_then(utils::color_to_hsla)
            .unwrap_or(white());
        let bg = popup_style
            .bg
            .and_then(utils::color_to_hsla)
            .unwrap_or(black());
        let mut style = Style::default();
        style.text.color = Some(fg);
        style.background = Some(bg.into());

        let info = cx.new_view(|cx| {
            let view = InfoBoxView::new(style);
            view.subscribe(&editor, cx);
            view
        });
        cx.subscribe(&info, |v, _e, _evt, cx| {
            v.info_hidden = true;
            cx.notify();
        })
        .detach();
        info
    }

    pub fn theme(editor: &Model<Core>, cx: &mut ViewContext<Self>) -> helix_view::Theme {
        editor.read(cx).editor.theme.clone()
    }

    pub fn handle_event(&mut self, ev: &crate::Update, cx: &mut ViewContext<Self>) {
        info!("handling event {:?}", ev);
        match ev {
            crate::Update::EditorEvent(ev) => {
                use helix_view::editor::EditorEvent;
                match ev {
                    EditorEvent::Redraw => cx.notify(),
                    EditorEvent::LanguageServerMessage(_) => { /* handled by notifications */ }
                    _ => {
                        info!("editor event {:?} not handled", ev);
                    }
                }
            }
            crate::Update::EditorStatus(_) => {}
            crate::Update::Redraw => {
                if let Some(view) = self.focused_view_id.and_then(|id| self.documents.get(&id)) {
                    view.update(cx, |_view, cx| {
                        cx.notify();
                    })
                }
                cx.notify();
            }
            crate::Update::Prompt(_) | crate::Update::Picker(_) => {
                // When a picker or prompt appears, auto-dismiss the info box
                self.info_hidden = true;
                // handled by overlay
                cx.notify();
            }
            crate::Update::Info(_) => {
                self.info_hidden = false;
                // handled by the info box view
            }
            crate::Update::ShouldQuit => {
                info!("ShouldQuit event received - triggering application quit");
                cx.quit();
            }
        }
    }

    fn handle_key(&mut self, ev: &KeyDownEvent, cx: &mut ViewContext<Self>) {
        // Check if we should dismiss the info box first
        if ev.keystroke.key == "escape" && !self.info_hidden {
            self.info_hidden = true;
            cx.notify();
            return; // Don't pass escape to editor when dismissing info box
        }

        // Check if overlay has a native picker - if so, don't consume key events
        // Let GPUI actions bubble up to the picker instead
        let overlay_view = &self.overlay.read(cx);
        if !overlay_view.is_empty() {
            // Check if it has a native picker by checking if it would render one
            // For now, just skip helix key processing when overlay is not empty
            // The picker will handle its own key events via GPUI actions
            println!("🚫 Skipping helix key processing - overlay active");
            return;
        }

        let key = utils::translate_key(&ev.keystroke);
        self.input.update(cx, |_, cx| {
            cx.emit(InputEvent::Key(key));
        })
    }

    fn make_views(
        &mut self,
        view_ids: &mut HashSet<ViewId>,
        right_borders: &mut HashSet<ViewId>,
        cx: &mut ViewContext<Self>,
    ) -> Option<String> {
        let editor = &self.core.read(cx).editor;
        let mut focused_file_name = None;

        for (view, is_focused) in editor.tree.views() {
            let view_id = view.id;

            if editor
                .tree
                .find_split_in_direction(view_id, helix_view::tree::Direction::Right)
                .is_some()
            {
                right_borders.insert(view_id);
            }

            view_ids.insert(view_id);

            if is_focused {
                let doc = editor.document(view.doc).unwrap();
                self.focused_view_id = Some(view_id);
                focused_file_name = doc.path().map(|p| p.display().to_string());
            }
        }

        for view_id in view_ids.iter() {
            let view_id = *view_id;
            let is_focused = self.focused_view_id == Some(view_id);
            let style = TextStyle {
                font_family: cx.global::<crate::FontSettings>().fixed_font.family.clone(),
                font_size: px(14.0).into(),
                ..Default::default()
            };
            let core = self.core.clone();
            let input = self.input.clone();
            let view = self.documents.entry(view_id).or_insert_with(|| {
                cx.new_view(|cx| {
                    DocumentView::new(
                        core,
                        input,
                        view_id,
                        style.clone(),
                        &cx.focus_handle(),
                        is_focused,
                    )
                })
            });
            view.update(cx, |view, _cx| {
                view.set_focused(is_focused);
            });
        }
        focused_file_name
    }
}

impl Render for Workspace {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl IntoElement {
        let mut view_ids = HashSet::new();
        let mut right_borders = HashSet::new();

        let focused_file_name = self.make_views(&mut view_ids, &mut right_borders, cx);

        let editor = &self.core.read(cx).editor;

        let default_style = editor.theme.get("ui.background");
        let default_ui_text = editor.theme.get("ui.text");
        let bg_color = utils::color_to_hsla(default_style.bg.unwrap()).unwrap_or(black());
        let text_color = utils::color_to_hsla(default_ui_text.fg.unwrap()).unwrap_or(white());
        let window_style = editor.theme.get("ui.window");
        let border_color = utils::color_to_hsla(window_style.fg.unwrap()).unwrap_or(white());

        let editor_rect = editor.tree.area();

        let editor = &self.core.read(cx).editor;
        let mut docs_root = div().flex().w_full().h_full();

        for (view, _) in editor.tree.views() {
            let view_id = view.id;
            if let Some(doc_view) = self.documents.get(&view_id) {
                let has_border = right_borders.contains(&view_id);
                let doc_element = div()
                    .flex()
                    .size_full()
                    .child(doc_view.clone())
                    .when(has_border, |this| {
                        this.border_color(border_color).border_r_1()
                    });
                docs_root = docs_root.child(doc_element);
            }
        }

        let to_remove = self
            .documents
            .keys()
            .copied()
            .filter(|id| !view_ids.contains(id))
            .collect::<Vec<_>>();
        for view_id in to_remove {
            if let Some(view) = self.documents.remove(&view_id) {
                cx.dismiss_view(&view);
            }
        }

        let focused_view = self
            .focused_view_id
            .and_then(|id| self.documents.get(&id))
            .cloned();
        if let Some(view) = &focused_view {
            cx.focus_view(view);
        }

        let label = if let Some(path) = focused_file_name {
            div()
                .flex_shrink()
                .font(cx.global::<crate::FontSettings>().var_font.clone())
                .text_color(text_color)
                .text_size(px(12.))
                .child(format!("{} - Helix", path))
        } else {
            div().flex()
        };
        let top_bar = div()
            .w_full()
            .flex()
            .flex_none()
            .h_8()
            .justify_center()
            .items_center()
            .child(label);

        self.core.update(cx, |core, _cx| {
            core.compositor.resize(editor_rect);
        });

        if let Some(view) = &focused_view {
            cx.focus_view(view);
        }

        div()
            .on_key_down(cx.listener(|view, ev, cx| {
                view.handle_key(ev, cx);
            }))
            .on_action(move |&crate::About, _cx| {
                eprintln!("hello");
            })
            .on_action({
                let handle = self.handle.clone();
                let core = self.core.clone();

                move |&crate::Quit, cx| {
                    eprintln!("quit?");
                    quit(core.clone(), handle.clone(), cx);
                    eprintln!("quit!");
                    cx.quit();
                }
            })
            .on_action({
                let handle = self.handle.clone();
                let core = self.core.clone();

                move |&crate::OpenFile, cx| {
                    info!("open file");
                    open(core.clone(), handle.clone(), cx)
                }
            })
            .on_action(move |&crate::Hide, cx| cx.hide())
            .on_action(move |&crate::HideOthers, cx| cx.hide_other_apps())
            .on_action(move |&crate::ShowAll, cx| cx.unhide_other_apps())
            .on_action(move |&crate::Minimize, cx| cx.minimize_window())
            .on_action(move |&crate::Zoom, cx| cx.zoom_window())
            .on_action({
                let handle = self.handle.clone();
                let core = self.core.clone();
                cx.listener(move |_, &crate::Tutor, cx| {
                    load_tutor(core.clone(), handle.clone(), cx)
                })
            })
            .on_action({
                let handle = self.handle.clone();
                let core = self.core.clone();
                cx.listener(move |_, &crate::TestPrompt, cx| {
                    test_prompt(core.clone(), handle.clone(), cx)
                })
            })
            .id("workspace")
            .bg(bg_color)
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .focusable()
            .child(top_bar)
            .when_some(Some(docs_root), |this, docs| this.child(docs))
            .child(self.notifications.clone())
            .when(!self.overlay.read(cx).is_empty(), |this| {
                let view = &self.overlay;
                cx.focus_view(&view);
                this.child(view.clone())
            })
            .when(
                !self.info_hidden && !self.info.read(cx).is_empty(),
                |this| this.child(self.info.clone()),
            )
    }
}

fn load_tutor(core: Model<Core>, handle: tokio::runtime::Handle, cx: &mut ViewContext<Workspace>) {
    core.update(cx, move |core, cx| {
        let _guard = handle.enter();
        let _ = utils::load_tutor(&mut core.editor);
        cx.notify()
    })
}

fn open(core: Model<Core>, handle: tokio::runtime::Handle, cx: &mut WindowContext) {
    // Create and emit a native file picker instead of using system dialog
    core.update(cx, move |core, cx| {
        let _guard = handle.enter();
        
        // Create a native file picker directly
        let native_picker = core.create_sample_native_file_picker();
        
        // Emit the picker to show it in the overlay
        cx.emit(crate::Update::Picker(native_picker));
    });
}

fn test_prompt(core: Model<Core>, handle: tokio::runtime::Handle, cx: &mut WindowContext) {
    // Create and emit a native prompt for testing
    core.update(cx, move |core, cx| {
        let _guard = handle.enter();
        
        // Create a native prompt directly
        let native_prompt = core.create_sample_native_prompt();
        
        // Emit the prompt to show it in the overlay
        cx.emit(crate::Update::Prompt(native_prompt));
    });
}

fn quit(core: Model<Core>, rt: tokio::runtime::Handle, cx: &mut WindowContext) {
    core.update(cx, |core, _cx| {
        let editor = &mut core.editor;
        let _guard = rt.enter();
        rt.block_on(async { editor.flush_writes().await }).unwrap();
        let views: Vec<_> = editor.tree.views().map(|(view, _)| view.id).collect();
        for view_id in views {
            editor.close(view_id);
        }
    });
}
