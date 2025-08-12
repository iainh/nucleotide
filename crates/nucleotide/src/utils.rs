// ABOUTME: Application-level utility functions
// ABOUTME: Keeps only app-specific utilities, others moved to appropriate layers

// Re-export utilities from lower layers
pub use nucleotide_core::utils::{detect_bundle_runtime, handle_key_result, translate_key};
pub use nucleotide_ui::theme_utils::color_to_hsla;

/// Load the Helix tutor document
pub fn load_tutor(editor: &mut helix_view::editor::Editor) -> Result<(), anyhow::Error> {
    use helix_core::{pos_at_coords, Position, Selection};
    use helix_view::doc_mut;
    use helix_view::editor::Action;
    use std::path::Path;

    let path = helix_loader::runtime_file(Path::new("tutor"));
    let doc_id = editor.open(&path, Action::VerticalSplit)?;
    let view_id = editor.tree.focus;
    // Check if the view exists before setting selection
    if editor.tree.contains(view_id) {
        let doc = doc_mut!(editor, &doc_id);
        let pos = Selection::point(pos_at_coords(
            doc.text().slice(..),
            Position::new(0, 0),
            true,
        ));
        doc.set_selection(view_id, pos);
    }

    // Unset path to prevent accidentally saving to the original tutor file.
    doc_mut!(editor).set_path(None);

    Ok(())
}
