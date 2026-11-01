//! Public-contract spike: real Editor/Document/Tree, no workspace or fake buffers.
use std::sync::Arc;

use arc_swap::ArcSwap;
use helix_core::{syntax, Transaction};
use helix_loader::workspace_trust::WorkspaceTrust;
use helix_view::{
    editor::{
        hosted::{DocumentReplacementReason, HostedEffect, SlotRemovalReason},
        Action, Config, LineEndingConfig,
    },
    graphics::Rect,
    handlers::{completion::CompletionHandler, word_index, Handlers},
    theme,
    tree::{Content, Layout},
    Editor,
};

fn editor() -> Editor {
    // No frontend hooks are registered. The editing/lifecycle boundary is real;
    // completion/diagnostics consumers are irrelevant to these contract tests.
    let handlers = Handlers {
        completions: CompletionHandler::new(tokio::sync::mpsc::channel(16).0),
        signature_hints: tokio::sync::mpsc::channel(16).0,
        auto_save: tokio::sync::mpsc::channel(16).0,
        document_colors: tokio::sync::mpsc::channel(16).0,
        document_links: tokio::sync::mpsc::channel(16).0,
        word_index: word_index::Handler::spawn(),
        pull_diagnostics: tokio::sync::mpsc::channel(16).0,
        pull_all_documents_diagnostics: tokio::sync::mpsc::channel(16).0,
        code_action_hint: tokio::sync::mpsc::channel(16).0,
    };
    Editor::new(
        Rect::new(0, 0, 180, 80),
        Arc::new(theme::Loader::new(&[])),
        Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        Arc::new(ArcSwap::from_pointee(Config {
            default_line_ending: LineEndingConfig::LF,
            ..Config::default()
        })),
        handlers,
        WorkspaceTrust::fully_trusted(),
    )
}

#[tokio::test]
async fn hosted_two_documents_and_content_only_pane() {
    let mut editor = editor();
    editor.enable_hosted();
    assert!(!editor.has_hosted_effects());
    let scratch = editor.new_file(Action::Replace);
    let left = editor.tree.focus;
    let document = editor.new_file(Action::VerticalSplit);
    let right = editor.tree.focus;
    editor
        .project_hosted_viewport(left, Rect::new(3, 7, 41, 19))
        .unwrap();
    editor
        .project_hosted_viewport(right, Rect::new(53, 11, 97, 31))
        .unwrap();
    editor.drain_hosted_effects();

    // The host's third pane is terminal/image-only: it has no ViewId or doc.
    editor.project_hosted_focus(None).unwrap();
    assert_eq!(editor.hosted_active_slot(), None);
    assert_eq!(editor.tree.views().count(), 2);
    assert_eq!(editor.documents.len(), 2);
    assert!(editor.drain_hosted_effects().is_empty());

    editor.project_hosted_focus(Some(left)).unwrap();
    editor.switch(
        scratch,
        Action::SplitAt {
            layout: Layout::Horizontal,
            before: true,
        },
    );
    let split = editor.tree.focus;
    assert_eq!(editor.hosted_active_slot(), Some(split));
    assert!(editor.has_hosted_effects());
    assert!(
        editor.has_hosted_effects(),
        "observing must not drain facts"
    );
    assert_eq!(
        editor.drain_hosted_effects(),
        vec![HostedEffect::SlotCreated {
            slot: split,
            document: scratch,
            source: Some(left),
            action: Action::SplitAt {
                layout: Layout::Horizontal,
                before: true
            },
        }]
    );

    // A following editor command sees the newly focused slot synchronously.
    let doc = editor.document_mut(scratch).unwrap();
    let transaction = Transaction::insert(doc.text(), doc.selection(split), "split edit".into());
    assert!(doc.apply(&transaction, split));
    editor.project_hosted_focus(Some(right)).unwrap();
    editor.switch(scratch, Action::Replace);
    assert_eq!(editor.tree.get(right).doc, scratch);
    // Helix's default scratch contains a newline, with the selection after it.
    assert_eq!(
        editor.document(scratch).unwrap().text().to_string(),
        "\nsplit edit"
    );
    editor.close_document(scratch, true).ok().unwrap();
    let effects = editor.drain_hosted_effects();
    assert!(effects.contains(&HostedEffect::DocumentReplaced {
        slot: right,
        previous: scratch,
        document,
        reason: DocumentReplacementReason::DocumentClose,
    }));
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(
                effect,
                HostedEffect::SlotRemoved {
                    reason: SlotRemovalReason::DocumentClose,
                    ..
                }
            ))
            .count(),
        2
    );
    assert!(!effects.iter().any(|effect| matches!(
        effect,
        HostedEffect::SlotRemoved {
            reason: SlotRemovalReason::ExplicitClose,
            ..
        }
    )));
    assert_eq!(
        effects.last(),
        Some(&HostedEffect::DocumentClosed { document: scratch })
    );
    assert!(editor.tree.try_get(left).is_none());
    assert!(editor.tree.try_get(split).is_none());
    assert_eq!(editor.tree.get(right).area, Rect::new(53, 11, 97, 31));
    editor.resize(Rect::new(0, 0, 250, 120));
    editor.tree.recalculate();
    editor.tree.transpose();
    assert_eq!(editor.tree.get(right).area, Rect::new(53, 11, 97, 31));

    // Final buffer close removes only its slot, not the host's content pane,
    // and does not manufacture a fallback scratch document/presentation.
    editor.close_document(document, true).ok().unwrap();
    assert_eq!(editor.tree.views().count(), 0);
    assert!(editor.documents.is_empty());
    assert_eq!(editor.hosted_active_slot(), None);
    assert!(!editor.should_close());
    assert!(
        editor.has_hosted_effects(),
        "buffer close must yield before a continuation"
    );
    assert_eq!(
        editor.drain_hosted_effects(),
        vec![
            HostedEffect::SlotRemoved {
                slot: right,
                document,
                reason: SlotRemovalReason::DocumentClose
            },
            HostedEffect::DocumentClosed { document },
        ]
    );
    assert!(!editor.has_hosted_effects());
    println!("spike: 2 real documents + content-only pane; exact split intent; edit/focus; buffer fallback distinguished; zero slots/documents; geometry preserved");
}

#[tokio::test]
async fn hosted_retains_scratch_and_projection_does_not_echo() {
    let mut editor = editor();
    editor.enable_hosted();
    let scratch = editor.new_file(Action::Replace);
    let slot = editor.tree.focus;
    let replacement = editor.new_file(Action::Load);
    assert_eq!(editor.tree.get(slot).doc, scratch);
    editor.drain_hosted_effects();
    editor.project_hosted_replace(slot, replacement).unwrap();
    assert!(
        editor.document(scratch).is_some(),
        "undisplayed empty scratch tab is real retained state"
    );
    editor.project_hosted_release(slot).unwrap();
    assert_eq!(editor.tree.views().count(), 0);
    assert_eq!(editor.documents.len(), 2);
    assert!(!editor.should_close());
    let restored = editor.project_hosted_slot(scratch).unwrap();
    assert_ne!(restored, slot);
    assert_eq!(editor.hosted_active_slot(), Some(restored));
    editor.project_hosted_focus(None).unwrap();
    assert!(
        !editor.has_hosted_effects(),
        "silent projection must not request reconciliation"
    );
    assert!(editor.drain_hosted_effects().is_empty());

    // An incidental async load/replace with terminal focus cannot mutate the
    // hidden real slot or inherit it as split source.
    editor.switch(replacement, Action::Load);
    editor.switch(replacement, Action::Replace);
    let added = editor.tree.focus;
    assert_eq!(editor.tree.get(restored).doc, scratch);
    assert_eq!(
        editor.drain_hosted_effects(),
        vec![
            HostedEffect::DocumentLoaded {
                document: replacement,
                slot: None
            },
            HostedEffect::SlotCreated {
                slot: added,
                document: replacement,
                source: None,
                action: Action::Replace
            },
        ]
    );
    editor.close(added);
    assert_eq!(
        editor.drain_hosted_effects(),
        vec![HostedEffect::SlotRemoved {
            slot: added,
            document: replacement,
            reason: SlotRemovalReason::ExplicitClose,
        }]
    );
}

#[tokio::test]
async fn standalone_keeps_scratch_cleanup_reflow_and_buffer_fallback() {
    let mut editor = editor();
    let scratch = editor.new_file(Action::VerticalSplit);
    let slot = editor.tree.focus;
    let document = editor.new_file(Action::Replace);
    assert!(editor.document(scratch).is_none());
    assert_eq!(editor.tree.get(slot).doc, document);
    editor.resize(Rect::new(0, 0, 93, 37));
    assert_eq!(editor.tree.get(slot).area, Rect::new(0, 0, 93, 37));
    editor.close_document(document, true).ok().unwrap();
    assert_eq!(editor.tree.views().count(), 1);
    assert_eq!(editor.documents.len(), 1);
    assert!(!editor.should_close());
    assert!(
        !editor.has_hosted_effects(),
        "standalone commands never queue hosted facts"
    );
    assert!(editor.drain_hosted_effects().is_empty());
    editor.close(editor.tree.focus);
    assert!(editor.should_close());
}

#[tokio::test]
async fn hosted_replace_preserves_history_jumps_mode_and_focus_hooks() {
    use helix_view::{document::Mode, events::DocumentFocusLost};
    use std::sync::Mutex;

    let mut editor = Box::new(editor());
    let target = (&*editor as *const Editor) as usize;
    let lost = Arc::new(Mutex::new(Vec::new()));
    let observed = lost.clone();
    helix_event::register_event::<DocumentFocusLost>();
    helix_event::register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        if (event.editor as *const Editor) as usize == target {
            observed.lock().unwrap().push(event.doc);
        }
        Ok(())
    });
    editor.enable_hosted();
    let original = editor.new_file(Action::Replace);
    let slot = editor.tree.focus;
    let doc = editor.document_mut(original).unwrap();
    let transaction = Transaction::insert(doc.text(), doc.selection(slot), "pending".into());
    doc.apply(&transaction, slot);
    editor.mode = Mode::Insert;
    let replacement = editor.new_file(Action::Load);
    editor.project_hosted_replace(slot, replacement).unwrap();
    assert_eq!(editor.mode(), Mode::Normal);
    // Keep the existing Replace hook contract, including its document argument.
    assert_eq!(*lost.lock().unwrap(), vec![replacement]);
    editor.jump_backward(slot, 1);
    assert_eq!(editor.tree.get(slot).doc, original);
    assert_eq!(
        editor.document(original).unwrap().text().to_string(),
        "\npending"
    );
    {
        let Editor {
            tree, documents, ..
        } = &mut *editor;
        assert!(documents
            .get_mut(&original)
            .unwrap()
            .undo(tree.get_mut(slot)));
    }
    assert_eq!(editor.document(original).unwrap().text().to_string(), "\n");
    lost.lock().unwrap().clear();
    editor.project_hosted_focus(None).unwrap();
    editor.project_hosted_focus(None).unwrap();
    editor.project_hosted_focus(Some(slot)).unwrap();
    assert_eq!(*lost.lock().unwrap(), vec![original]);
}

#[tokio::test]
async fn hosted_replacement_from_content_focus_preserves_retained_document_state() {
    use helix_core::Selection;
    use helix_view::view::ViewPosition;

    let root = tempfile::tempdir().unwrap();
    let paths = ["A.txt", "B.txt"].map(|name| root.path().join(name));
    let text: String = (0..40)
        .map(|line| format!("line-{line:02} retained state\n"))
        .collect();
    for path in &paths {
        std::fs::write(path, &text).unwrap();
    }
    let mut editor = editor();
    let a = editor.open(&paths[0], Action::VerticalSplit).unwrap();
    let slot = editor.tree.focus;
    let selection = Selection::single(59, 65);
    let offset = ViewPosition {
        anchor: 46,
        horizontal_offset: 3,
        vertical_offset: 0,
    };
    let doc = editor.document_mut(a).unwrap();
    doc.set_selection(slot, selection.clone());
    doc.set_view_offset(slot, offset);
    editor.enable_hosted();
    editor.project_hosted_focus(None).unwrap();
    let b = editor.open(&paths[1], Action::Replace).unwrap();
    let incoming = editor.tree.focus;
    assert_ne!(incoming, slot);
    assert_eq!(editor.document(a).unwrap().view_offset(slot), offset);

    let extra = editor.project_hosted_slot(b).unwrap();
    assert_eq!(editor.document(a).unwrap().view_offset(slot), offset);
    editor.project_hosted_focus(None).unwrap();
    editor.project_hosted_release(extra).unwrap();
    assert_eq!(editor.document(a).unwrap().view_offset(slot), offset);
    editor.close_document(b, true).ok().unwrap();
    assert!(editor.tree.try_get(incoming).is_none());
    assert_eq!(editor.hosted_active_slot(), None);
    assert_eq!(editor.document(a).unwrap().view_offset(slot), offset);

    let b = editor.open(&paths[1], Action::Load).unwrap();
    assert!(!editor.document(b).unwrap().selections().contains_key(&slot));
    editor.drain_hosted_effects();

    editor.project_hosted_replace(slot, b).unwrap();
    assert_eq!(
        editor.document(b).unwrap().view_offset(slot),
        ViewPosition::default()
    );
    editor.project_hosted_replace(slot, a).unwrap();
    assert_eq!(editor.document(a).unwrap().selection(slot), &selection);
    assert_eq!(editor.document(a).unwrap().view_offset(slot), offset);
    assert!(!editor.has_hosted_effects());
}

#[tokio::test]
async fn refresh_reveals_the_active_hosted_slot_but_preserves_hidden_viewports() {
    use helix_core::Selection;
    use helix_view::view::ViewPosition;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("refresh.txt");
    let text: String = (0..40)
        .map(|line| format!("line-{line:02} retained state\n"))
        .collect();
    std::fs::write(&path, text).unwrap();
    for hosted in [false, true] {
        let mut editor = editor();
        let document = editor.open(&path, Action::VerticalSplit).unwrap();
        let hidden = editor.tree.focus;
        editor.switch(document, Action::VerticalSplit);
        let active = editor.tree.focus;
        let offset = ViewPosition {
            anchor: 46,
            horizontal_offset: 3,
            vertical_offset: 0,
        };
        let selection = Selection::single(59, 65);
        let doc = editor.document_mut(document).unwrap();
        doc.set_selection(hidden, selection.clone());
        doc.set_view_offset(hidden, offset);
        doc.set_selection(active, Selection::point(0));
        doc.set_view_offset(active, offset);
        if hosted {
            editor.enable_hosted();
        }
        editor.resize(Rect::new(0, 0, 180, 81));
        let doc = editor.document(document).unwrap();
        assert_eq!(doc.selection(hidden), &selection);
        assert_eq!(doc.view_offset(active), ViewPosition::default());
        if hosted {
            assert_eq!(doc.view_offset(hidden), offset);
        } else {
            assert_eq!(
                doc.view_offset(hidden).anchor,
                0,
                "standalone still reveals every view"
            );
        }
    }
}

#[tokio::test]
async fn hosted_replacement_preserves_other_retained_tabs_when_focusing_an_inactive_slot() {
    use helix_core::Selection;
    use helix_view::view::ViewPosition;

    let root = tempfile::tempdir().unwrap();
    let paths = ["A.txt", "B.txt", "C.txt"].map(|name| root.path().join(name));
    for (index, path) in paths.iter().enumerate() {
        let text: String = (0..30)
            .map(|line| format!("document-{index} line-{line:02} state\n"))
            .collect();
        std::fs::write(path, text).unwrap();
    }
    let mut editor = editor();
    let a = editor.open(&paths[0], Action::VerticalSplit).unwrap();
    let source = editor.tree.focus;
    let b = editor.open(&paths[1], Action::Replace).unwrap();
    let b_selection = Selection::single(12, 17);
    editor
        .document_mut(b)
        .unwrap()
        .set_selection(source, b_selection.clone());
    let c = editor.open(&paths[2], Action::VerticalSplit).unwrap();
    let target = editor.tree.focus;
    let c_selection = Selection::single(108, 102);
    let c_offset = ViewPosition {
        anchor: 75,
        horizontal_offset: 7,
        vertical_offset: 0,
    };
    let doc = editor.document_mut(c).unwrap();
    doc.set_selection(target, c_selection.clone());
    doc.set_view_offset(target, c_offset);
    editor.enable_hosted();
    editor.project_hosted_focus(Some(source)).unwrap();

    // The host has transferred inactive A's per-slot state into the destination.
    let a_selection = Selection::single(83, 77);
    let a_offset = ViewPosition {
        anchor: 50,
        horizontal_offset: 4,
        vertical_offset: 0,
    };
    let doc = editor.document_mut(a).unwrap();
    doc.ensure_view_init(target);
    doc.set_selection(target, a_selection.clone());
    doc.set_view_offset(target, a_offset);
    editor.project_hosted_replace(target, a).unwrap();
    assert_eq!(editor.document(a).unwrap().selection(target), &a_selection);
    assert_eq!(editor.document(a).unwrap().view_offset(target), a_offset);
    editor.project_hosted_replace(target, c).unwrap();
    assert_eq!(editor.document(c).unwrap().selection(target), &c_selection);
    assert_eq!(editor.document(c).unwrap().view_offset(target), c_offset);
    assert_eq!(editor.document(b).unwrap().selection(source), &b_selection);
    assert!(!editor.has_hosted_effects());
}

#[tokio::test]
async fn topology_import_preserves_regular_split_and_split_at_distinction() {
    let mut editor = editor();
    let document = editor.new_file(Action::VerticalSplit);
    let left = editor.tree.focus;
    editor.switch(document, Action::VerticalSplit);
    let right = editor.tree.focus;
    editor.focus(left);
    editor.switch(
        document,
        Action::SplitAt {
            layout: Layout::Vertical,
            before: true,
        },
    );
    let inserted = editor.tree.focus;
    let Some(Content::Container(root)) = editor.tree.content(editor.tree.root()) else {
        panic!()
    };
    assert_eq!(root.layout(), Layout::Vertical);
    assert_eq!(root.area(), Rect::new(0, 0, 180, 79));
    assert_eq!(
        root.children().len(),
        2,
        "SplitAt must not redistribute the root's siblings"
    );
    assert_eq!(root.children()[1], right);
    let Some(Content::Container(split)) = editor.tree.content(root.children()[0]) else {
        panic!()
    };
    assert_eq!(split.layout(), Layout::Vertical);
    assert_eq!(split.children(), &[inserted, left]);
    assert_eq!(editor.tree.get(right).area, Rect::new(91, 0, 89, 79));
    editor.focus(right);
    editor.switch(document, Action::VerticalSplit);
    let Some(Content::Container(root)) = editor.tree.content(editor.tree.root()) else {
        panic!()
    };
    assert_eq!(
        root.children().len(),
        3,
        "regular split redistributes matching-axis siblings"
    );
    assert_eq!(editor.tree.get(right).area, Rect::new(60, 0, 59, 79));
}
