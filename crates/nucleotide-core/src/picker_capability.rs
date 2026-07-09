/// Optional capability used by UI pickers to render file previews.
pub trait PickerCapability {
    fn render_preview(
        &self,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    ) -> gpui::AnyElement;
}
