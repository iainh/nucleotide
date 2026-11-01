//! Early real-editor seam scenario. This does not stand in for native GUI or
//! terminal evidence; integration registers it after installing PaneHost.
use super::pane_host::PaneHost;
use super::pane_layout::{Edge, PaneSize, Rect as LayoutRect, SplitMode};
use arc_swap::{ArcSwap, access::Map};
use helix_core::syntax;
use helix_view::{
    Editor,
    editor::{Action, Config},
    graphics::Rect,
    handlers::{Handlers, completion::CompletionHandler, word_index::Handler as WordIndexHandler},
    theme,
};
use std::sync::Arc;

fn real_editor() -> Editor {
    let (completion_tx, _) = tokio::sync::mpsc::channel(1);
    let (signature_tx, _) = tokio::sync::mpsc::channel(1);
    let (auto_save_tx, _) = tokio::sync::mpsc::channel(1);
    let (doc_colors_tx, _) = tokio::sync::mpsc::channel(1);
    let (doc_links_tx, _) = tokio::sync::mpsc::channel(1);
    let (pull_diagnostics_tx, _) = tokio::sync::mpsc::channel(1);
    let (pull_all_diagnostics_tx, _) = tokio::sync::mpsc::channel(1);
    let (code_action_hint_tx, _) = tokio::sync::mpsc::channel(1);
    let config = Arc::new(ArcSwap::from_pointee(Config::default()));
    Editor::new(
        Rect::new(0, 0, 120, 40),
        Arc::new(theme::Loader::new(&[])),
        Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        Arc::new(Map::new(config, |config: &Config| config)),
        Handlers {
            completions: CompletionHandler::new(completion_tx),
            signature_hints: signature_tx,
            auto_save: auto_save_tx,
            document_colors: doc_colors_tx,
            document_links: doc_links_tx,
            word_index: WordIndexHandler::spawn(),
            pull_diagnostics: pull_diagnostics_tx,
            pull_all_documents_diagnostics: pull_all_diagnostics_tx,
            code_action_hint: code_action_hint_tx,
        },
        helix_loader::workspace_trust::WorkspaceTrust::fully_trusted(),
    )
}

#[tokio::test]
async fn content_only_focus_and_final_document_close_preserve_workspace_without_reflow() {
    let mut editor = real_editor();
    let first_document = editor.new_file(Action::VerticalSplit);
    let first_slot = editor.tree.focus;
    let second_document = editor.new_file(Action::VerticalSplit);
    let second_slot = editor.tree.focus;
    let mut host = PaneHost::import(&editor);
    assert_eq!(host.slots.len(), 2);
    assert_ne!(first_document, second_document);
    editor.enable_hosted();

    // Deliberately asymmetric projected viewports detect accidental tree reflow
    // when the third GUI pane has no Helix view. No fake image ID is required.
    let first_area = Rect::new(3, 5, 37, 13);
    let second_area = Rect::new(59, 2, 23, 29);
    editor
        .project_hosted_viewport(first_slot, first_area)
        .unwrap();
    editor
        .project_hosted_viewport(second_slot, second_area)
        .unwrap();
    let content_pane = host
        .layout
        .split_pane(
            host.layout.active_pane(),
            Edge::Bottom,
            SplitMode::TargetOnly,
            Vec::new(),
        )
        .unwrap();
    host.layout.activate_pane(content_pane).unwrap();
    let bounds = LayoutRect {
        x: 7.5,
        y: 11.25,
        width: 997.5,
        height: 613.75,
    };
    let geometry = host.layout.layout(bounds, PaneSize::default()).unwrap();
    host.project(&mut editor).unwrap();
    assert_eq!(geometry.len(), 3);
    assert_eq!(host.layout.panes().len(), 3);
    assert_eq!(host.slots.len(), 2);
    assert!(!host.slots.contains_key(&content_pane));
    assert_eq!(editor.documents.len(), 2);
    assert_eq!(editor.tree.views().count(), 2);
    assert_eq!(host.active_slot(), None);
    assert_eq!(editor.hosted_active_slot(), None);
    assert_eq!(editor.tree.get(first_slot).area, first_area);
    assert_eq!(editor.tree.get(second_slot).area, second_area);
    assert!(!editor.should_close());
    assert!(editor.drain_hosted_effects().is_empty());

    assert!(editor.close_document(first_document, true).is_ok());
    host.remove_document(first_document);
    host.project(&mut editor).unwrap();
    assert_eq!(host.slots.len(), 1);
    assert_eq!(host.layout.panes().len(), 2);
    assert_eq!(editor.tree.get(second_slot).area, second_area);
    assert_eq!(host.layout.active_pane(), content_pane);
    assert!(!editor.should_close());

    assert!(editor.close_document(second_document, true).is_ok());
    host.remove_document(second_document);
    host.project(&mut editor).unwrap();
    assert!(editor.documents.is_empty());
    assert_eq!(editor.tree.views().count(), 0);
    assert!(host.slots.is_empty());
    assert_eq!(host.active_slot(), None);
    assert_eq!(editor.hosted_active_slot(), None);
    assert_eq!(host.layout.active_pane(), content_pane);
    assert_eq!(host.layout.panes(), vec![content_pane]);
    let remaining = host.layout.layout(bounds, PaneSize::default()).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].pane, content_pane);
    assert_eq!(remaining[0].rect, bounds);
    host.layout.validate().unwrap();
    assert!(!editor.should_close());
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_documents_from_active_image_keeps_every_real_slot_owned() {
    use crate::tab::TabId;
    use helix_core::Selection;
    use helix_view::view::ViewPosition;
    use std::collections::HashSet;

    let root = tempfile::tempdir().unwrap();
    let paths = ["A.txt", "B.txt", "C.txt"].map(|name| root.path().join(name));
    let text: String = (0..40)
        .map(|line| format!("line-{line:02} retained state\n"))
        .collect();
    for path in &paths {
        std::fs::write(path, &text).unwrap();
    }
    let mut editor = real_editor();
    let original = editor.open(&paths[0], Action::VerticalSplit).unwrap();
    let original_slot = editor.tree.focus;
    let selection = Selection::single(59, 65);
    let offset = ViewPosition {
        anchor: editor.documents[&original].text().line_to_char(2),
        horizontal_offset: 3,
        vertical_offset: 0,
    };
    editor
        .documents
        .get_mut(&original)
        .unwrap()
        .set_selection(original_slot, selection.clone());
    editor
        .documents
        .get_mut(&original)
        .unwrap()
        .set_view_offset(original_slot, offset);
    let mut host = PaneHost::import(&editor);
    let pane = host.pane_for_slot(original_slot).unwrap();
    editor.enable_hosted();
    let image = TabId::Image(super::ImageId(701));
    host.layout.open_tab(pane, image, true).unwrap();
    let mut jobs = helix_term::job::Jobs::new();

    for path in &paths[1..] {
        host.layout.activate_tab(pane, image).unwrap();
        host.project(&mut editor).unwrap();
        assert_eq!(editor.hosted_active_slot(), None);
        let argument = path.to_string_lossy().replace('\\', "/");
        crate::helix_command::execute_command_line(
            &mut editor,
            &mut jobs,
            &format!("open '{argument}'"),
        );
        host.apply_effects(editor.drain_hosted_effects()).unwrap();
        host.project(&mut editor).unwrap();
        let real: HashSet<_> = editor.tree.views().map(|(view, _)| view.id).collect();
        let owned: HashSet<_> = host.slots.values().copied().collect();
        assert_eq!(
            real, owned,
            "opening from content focus must not orphan a real slot"
        );
        assert_eq!(
            real.len(),
            1,
            "one mixed pane needs only one retained editing slot"
        );
        assert_eq!(host.layout.panes(), vec![pane]);
        host.layout
            .activate_tab(pane, TabId::Document(original))
            .unwrap();
        host.project(&mut editor).unwrap();
        let retained = host.slots[&pane];
        assert_eq!(editor.documents[&original].selection(retained), &selection);
        assert_eq!(editor.documents[&original].view_offset(retained), offset);
        assert_eq!(editor.documents[&original].text().to_string(), text);
        host.layout.validate().unwrap();
    }
    assert_eq!(editor.documents.len(), 3);
}

#[tokio::test]
async fn moving_inactive_document_preserves_its_state_without_clobbering_other_tabs() {
    use crate::tab::TabId;
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
    let mut editor = real_editor();
    let a = editor.open(&paths[0], Action::VerticalSplit).unwrap();
    let source_slot = editor.tree.focus;
    let a_selection = Selection::single(83, 77);
    let a_offset = ViewPosition {
        anchor: 50,
        horizontal_offset: 4,
        vertical_offset: 0,
    };
    editor
        .documents
        .get_mut(&a)
        .unwrap()
        .set_selection(source_slot, a_selection.clone());
    editor
        .documents
        .get_mut(&a)
        .unwrap()
        .set_view_offset(source_slot, a_offset);
    let b = editor.open(&paths[1], Action::Replace).unwrap();
    let b_selection = Selection::single(12, 17);
    editor
        .documents
        .get_mut(&b)
        .unwrap()
        .set_selection(source_slot, b_selection.clone());
    let c = editor.open(&paths[2], Action::VerticalSplit).unwrap();
    let target_slot = editor.tree.focus;
    let c_selection = Selection::single(108, 102);
    let c_offset = ViewPosition {
        anchor: 75,
        horizontal_offset: 7,
        vertical_offset: 0,
    };
    editor
        .documents
        .get_mut(&c)
        .unwrap()
        .set_selection(target_slot, c_selection.clone());
    editor
        .documents
        .get_mut(&c)
        .unwrap()
        .set_view_offset(target_slot, c_offset);
    let mut host = PaneHost::import(&editor);
    let source = host.pane_for_slot(source_slot).unwrap();
    let target = host.pane_for_slot(target_slot).unwrap();
    host.layout
        .open_tab(source, TabId::Document(a), false)
        .unwrap();
    host.layout.close_tab(target, TabId::Document(a)).unwrap();
    editor.enable_hosted();
    host.project(&mut editor).unwrap();
    assert_eq!(editor.tree.get(source_slot).doc, b);
    assert_eq!(editor.tree.get(target_slot).doc, c);

    host.transfer_state(source, target);
    host.layout
        .transfer_tab(source, TabId::Document(a), target, 0)
        .unwrap();
    host.project(&mut editor).unwrap();
    let moved_slot = host.slots[&target];
    assert_eq!(editor.tree.get(moved_slot).doc, a);
    assert_eq!(editor.documents[&a].selection(moved_slot), &a_selection);
    assert_eq!(editor.documents[&a].view_offset(moved_slot), a_offset);
    assert_eq!(editor.documents[&b].selection(source_slot), &b_selection);
    host.layout
        .activate_tab(target, TabId::Document(c))
        .unwrap();
    host.project(&mut editor).unwrap();
    assert_eq!(editor.documents[&c].selection(moved_slot), &c_selection);
    assert_eq!(editor.documents[&c].view_offset(moved_slot), c_offset);

    // A destination copy already has its own state: dedup must prefer it,
    // including when the transferred source document is not currently shown.
    let target_selection = Selection::single(154, 149);
    let target_offset = ViewPosition {
        anchor: 100,
        horizontal_offset: 2,
        vertical_offset: 0,
    };
    editor
        .documents
        .get_mut(&a)
        .unwrap()
        .set_selection(moved_slot, target_selection.clone());
    editor
        .documents
        .get_mut(&a)
        .unwrap()
        .set_view_offset(moved_slot, target_offset);
    host.layout
        .open_tab(source, TabId::Document(a), false)
        .unwrap();
    host.transfer_state(source, target);
    host.layout
        .transfer_tab(source, TabId::Document(a), target, 0)
        .unwrap();
    host.project(&mut editor).unwrap();
    assert_eq!(
        editor.documents[&a].selection(moved_slot),
        &target_selection
    );
    assert_eq!(editor.documents[&a].view_offset(moved_slot), target_offset);
    assert_eq!(editor.documents[&b].selection(source_slot), &b_selection);
    assert_eq!(editor.documents[&c].selection(moved_slot), &c_selection);
    assert_eq!(editor.documents[&c].view_offset(moved_slot), c_offset);
    assert_eq!(editor.tree.views().count(), 2);
    host.layout.validate().unwrap();
}

#[tokio::test]
async fn split_move_and_dedup_preserve_the_correct_editing_state_and_history() {
    use crate::tab::TabId;
    use helix_core::{Selection, Transaction};
    use helix_view::{
        ViewId,
        view::{JumpList, ViewPosition},
    };

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.txt");
    let original: String = (0..40)
        .map(|line| format!("row-{line:02}: distinct editor state\n"))
        .collect();
    std::fs::write(&path, &original).unwrap();
    let mut editor = real_editor();
    let document = editor.open(&path, Action::VerticalSplit).unwrap();
    let source_slot = editor.tree.focus;
    let transaction = Transaction::change(
        editor.documents[&document].text(),
        std::iter::once((0, 0, Some("pending history\n".into()))),
    );
    let doc = editor.documents.get_mut(&document).unwrap();
    assert!(doc.apply(&transaction, source_slot));
    doc.append_changes_to_history(editor.tree.get_mut(source_slot));
    let source_line = doc.text().line_to_char(10);
    let target_line = doc.text().line_to_char(18);
    let source_selection = Selection::single(source_line + 8, source_line + 3);
    let source_offset = ViewPosition {
        anchor: doc.text().line_to_char(2),
        horizontal_offset: 0,
        vertical_offset: 0,
    };
    let target_selection = Selection::single(target_line + 2, target_line + 11);
    let target_offset = ViewPosition {
        anchor: doc.text().line_to_char(7),
        horizontal_offset: 0,
        vertical_offset: 0,
    };
    doc.set_selection(source_slot, source_selection.clone());
    doc.set_view_offset(source_slot, source_offset);
    let source_jump = Selection::point(2);
    let target_jump = Selection::point(9);
    editor.tree.get_mut(source_slot).jumps = JumpList::new((document, source_jump.clone()));
    let mut host = PaneHost::import(&editor);
    let source = host.pane_for_slot(source_slot).unwrap();
    editor.enable_hosted();

    let assert_state = |editor: &Editor,
                        slot: ViewId,
                        selection: &Selection,
                        offset: ViewPosition,
                        jump: &Selection| {
        assert_eq!(editor.tree.get(slot).doc, document);
        assert_eq!(editor.documents[&document].selection(slot), selection);
        assert_eq!(editor.documents[&document].view_offset(slot), offset);
        assert_eq!(
            editor
                .tree
                .get(slot)
                .jumps
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec![(document, jump.clone())]
        );
    };
    let duplicate = host
        .layout
        .split_pane(
            source,
            Edge::Right,
            SplitMode::TargetOnly,
            vec![TabId::Document(document)],
        )
        .unwrap();
    host.transfer_state(source, duplicate);
    host.project(&mut editor).unwrap();
    let duplicate_slot = host.slots[&duplicate];
    assert_ne!(duplicate_slot, source_slot);
    assert_state(
        &editor,
        duplicate_slot,
        &source_selection,
        source_offset,
        &source_jump,
    );

    // Distinct target/source state makes a wrong dedup overwrite observable.
    let doc = editor.documents.get_mut(&document).unwrap();
    doc.set_selection(duplicate_slot, target_selection.clone());
    doc.set_view_offset(duplicate_slot, target_offset);
    editor.tree.get_mut(duplicate_slot).jumps = JumpList::new((document, target_jump.clone()));
    assert_state(
        &editor,
        source_slot,
        &source_selection,
        source_offset,
        &source_jump,
    );
    host.transfer_state(source, duplicate);
    host.layout
        .transfer_tab(source, TabId::Document(document), duplicate, 0)
        .unwrap();
    host.project(&mut editor).unwrap();
    assert!(host.layout.pane(source).is_none());
    assert!(!editor.tree.contains(source_slot));
    assert_eq!(host.slots[&duplicate], duplicate_slot);
    assert_state(
        &editor,
        duplicate_slot,
        &target_selection,
        target_offset,
        &target_jump,
    );

    // The final source placement collapses, so copying after releasing its slot
    // would lose selections/offsets. The empty destination must get a real slot.
    let destination = host
        .layout
        .split_pane(duplicate, Edge::Bottom, SplitMode::TargetOnly, Vec::new())
        .unwrap();
    host.transfer_state(duplicate, destination);
    host.layout
        .transfer_tab(duplicate, TabId::Document(document), destination, 0)
        .unwrap();
    host.project(&mut editor).unwrap();
    let moved_slot = host.slots[&destination];
    assert_ne!(moved_slot, duplicate_slot);
    assert!(!editor.tree.contains(duplicate_slot));
    assert_state(
        &editor,
        moved_slot,
        &target_selection,
        target_offset,
        &target_jump,
    );
    assert_eq!(host.layout.panes(), vec![destination]);
    assert_eq!(editor.documents.len(), 1);
    assert_eq!(editor.tree.views().count(), 1);
    assert_eq!(host.active_slot(), Some(moved_slot));
    host.layout.validate().unwrap();

    let mut jobs = helix_term::job::Jobs::new();
    crate::helix_command::execute_command_line(&mut editor, &mut jobs, "undo");
    assert_eq!(editor.documents[&document].text().to_string(), original);
    assert!(!editor.should_close());
}

#[tokio::test(flavor = "multi_thread")]
async fn explicit_buffer_close_validates_all_copies_without_closing_unrelated_content() {
    use crate::tab::TabId;
    use helix_core::Transaction;
    use helix_term::job::Jobs;
    use std::fs;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("shared.txt");
    let unrelated_path = root.path().join("unrelated.txt");
    fs::write(&path, "shared\n").unwrap();
    fs::write(&unrelated_path, "keep me\n").unwrap();
    let mut editor = real_editor();
    let shared = editor.open(&path, Action::VerticalSplit).unwrap();
    let first_slot = editor.tree.focus;
    editor.switch(shared, Action::VerticalSplit);
    let unrelated = editor.open(&unrelated_path, Action::VerticalSplit).unwrap();
    let mut host = PaneHost::import(&editor);
    editor.enable_hosted();
    let content_pane = host
        .layout
        .split_pane(
            host.layout.active_pane(),
            Edge::Bottom,
            SplitMode::TargetOnly,
            Vec::new(),
        )
        .unwrap();
    let shared_pane = host.pane_for_slot(first_slot).unwrap();
    host.layout.activate_pane(shared_pane).unwrap();
    host.project(&mut editor).unwrap();
    let transaction = Transaction::change(
        editor.documents[&shared].text(),
        std::iter::once((0, 0, Some("unsaved ".into()))),
    );
    assert!(
        editor
            .documents
            .get_mut(&shared)
            .unwrap()
            .apply(&transaction, first_slot)
    );
    let before: Vec<_> = host
        .layout
        .panes()
        .into_iter()
        .map(|pane| (pane, host.layout.pane(pane).unwrap().tabs().to_vec()))
        .collect();
    let mut jobs = Jobs::new();
    let target = editor.documents[&shared]
        .path()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    crate::helix_command::execute_command_line(
        &mut editor,
        &mut jobs,
        &format!("bclose '{target}'"),
    );
    host.apply_effects(editor.drain_hosted_effects()).unwrap();
    host.project(&mut editor).unwrap();
    let after: Vec<_> = host
        .layout
        .panes()
        .into_iter()
        .map(|pane| (pane, host.layout.pane(pane).unwrap().tabs().to_vec()))
        .collect();
    assert_eq!(after, before, "refused close must preserve every placement");
    assert_eq!(host.slots.len(), 3);
    assert_eq!(
        editor.documents[&shared].text().to_string(),
        "unsaved shared\n"
    );
    assert!(!editor.should_close());

    crate::helix_command::execute_command_line(
        &mut editor,
        &mut jobs,
        &format!("bclose! '{target}'"),
    );
    host.apply_effects(editor.drain_hosted_effects()).unwrap();
    host.project(&mut editor).unwrap();
    assert!(!editor.documents.contains_key(&shared));
    assert!(host.layout.panes().iter().all(|pane| {
        !host
            .layout
            .pane(*pane)
            .unwrap()
            .tabs()
            .contains(&TabId::Document(shared))
    }));
    assert!(host.layout.pane(content_pane).is_some());
    assert_eq!(host.slots.len(), 1);
    assert_eq!(editor.tree.views().count(), 1);
    assert_eq!(editor.tree.views().next().unwrap().0.doc, unrelated);
    assert_eq!(editor.documents[&unrelated].text().to_string(), "keep me\n");
    assert_eq!(fs::read_to_string(path).unwrap(), "shared\n");
    host.layout.validate().unwrap();
    assert!(!editor.should_close());
}
