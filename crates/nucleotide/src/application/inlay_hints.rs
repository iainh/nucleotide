//! Visible-view inlay hint requests and stale-safe response application.

use std::future::Future;
use std::pin::Pin;

use helix_core::{Range, syntax, text_annotations::InlineAnnotation};
use helix_lsp::{OffsetEncoding, lsp};
use helix_view::{
    Document, DocumentId, Editor, ViewId,
    document::{DocumentInlayHints, DocumentInlayHintsId},
    view::View,
};
use nucleotide_logging::trace;

use super::{Application, document_lsp_identifier};

type InlayHintJobFuture =
    Pin<Box<dyn Future<Output = anyhow::Result<helix_term::job::Callback>> + Send>>;

fn inlay_hint_jobs_for_visible_views(editor: &Editor) -> Vec<InlayHintJobFuture> {
    if !editor.config().lsp.display_inlay_hints {
        return Vec::new();
    }

    editor
        .tree
        .views()
        .filter_map(|(view, _)| {
            let doc = editor.documents.get(&view.doc)?;
            inlay_hint_job_for_view(view, doc)
        })
        .collect()
}

fn inlay_hint_job_for_view(view: &View, doc: &Document) -> Option<InlayHintJobFuture> {
    let view_id = view.id;
    let doc_id = view.doc;
    let language_server = doc
        .language_servers_with_feature(syntax::config::LanguageServerFeature::InlayHints)
        .next()?;

    let doc_text = doc.text();
    let len_lines = doc_text.len_lines();
    let view_height = view.inner_height();
    let first_visible_line =
        doc_text.char_to_line(doc.view_offset(view_id).anchor.min(doc_text.len_chars()));
    let first_line = first_visible_line.saturating_sub(view_height);
    let last_line = first_visible_line
        .saturating_add(view_height.saturating_mul(2))
        .min(len_lines);

    let inlay_hints_id = DocumentInlayHintsId {
        first_line,
        last_line,
    };
    let document_version = doc.version();
    if !doc.inlay_hints_oudated
        && doc
            .inlay_hints(view_id)
            .is_some_and(|hints| hints.id == inlay_hints_id)
    {
        return None;
    }

    let first_char_in_range = doc_text.slice(..).line_to_char(first_line);
    let last_char_in_range = doc_text.slice(..).line_to_char(last_line);
    let offset_encoding = language_server.offset_encoding();
    let range = helix_lsp::util::range_to_lsp_range(
        doc_text,
        Range::new(first_char_in_range, last_char_in_range),
        offset_encoding,
    );
    let identifier = document_lsp_identifier(doc)?;
    let request = language_server.text_document_range_inlay_hints(identifier, range, None)?;

    Some(Box::pin(async move {
        let response = request.await?;
        Ok(helix_term::job::Callback::Editor(Box::new(
            move |editor: &mut Editor| {
                apply_inlay_hint_response(
                    editor,
                    view_id,
                    doc_id,
                    inlay_hints_id,
                    document_version,
                    offset_encoding,
                    response,
                );
            },
        )))
    }))
}

fn apply_inlay_hint_response(
    editor: &mut Editor,
    view_id: ViewId,
    doc_id: DocumentId,
    inlay_hints_id: DocumentInlayHintsId,
    document_version: i32,
    offset_encoding: OffsetEncoding,
    response: Option<Vec<lsp::InlayHint>>,
) {
    if !editor.config().lsp.display_inlay_hints {
        return;
    }

    let Some(view_doc_id) = editor.tree.try_get(view_id).map(|view| view.doc) else {
        return;
    };
    if view_doc_id != doc_id {
        trace!(
            view_id = ?view_id,
            expected_doc_id = ?doc_id,
            actual_doc_id = ?view_doc_id,
            "Ignoring inlay hint response for stale view/document association"
        );
        return;
    }

    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };
    if doc.version() != document_version {
        trace!(
            doc_id = ?doc_id,
            request_version = document_version,
            current_version = doc.version(),
            "Ignoring stale inlay hint response for changed document"
        );
        return;
    }

    let mut hints = match response {
        Some(hints) if !hints.is_empty() => hints,
        _ => {
            doc.set_inlay_hints(view_id, DocumentInlayHints::empty_with_id(inlay_hints_id));
            doc.inlay_hints_oudated = false;
            return;
        }
    };

    hints.sort_by_key(|inlay_hint| inlay_hint.position);

    let mut padding_before_inlay_hints = Vec::new();
    let mut type_inlay_hints = Vec::new();
    let mut parameter_inlay_hints = Vec::new();
    let mut other_inlay_hints = Vec::new();
    let mut padding_after_inlay_hints = Vec::new();
    let doc_text = doc.text();
    let inlay_hints_length_limit = doc.config.load().lsp.inlay_hints_length_limit;

    for hint in hints {
        let Some(char_idx) =
            helix_lsp::util::lsp_pos_to_pos(doc_text, hint.position, offset_encoding)
        else {
            continue;
        };

        let mut label = match hint.label {
            lsp::InlayHintLabel::String(s) => s,
            lsp::InlayHintLabel::LabelParts(parts) => parts
                .into_iter()
                .map(|part| part.value)
                .collect::<Vec<_>>()
                .join(""),
        };
        truncate_inlay_hint_label(&mut label, inlay_hints_length_limit);

        let target = match hint.kind {
            Some(lsp::InlayHintKind::TYPE) => &mut type_inlay_hints,
            Some(lsp::InlayHintKind::PARAMETER) => &mut parameter_inlay_hints,
            _ => &mut other_inlay_hints,
        };

        if let Some(true) = hint.padding_left {
            padding_before_inlay_hints.push(InlineAnnotation::new(char_idx, " "));
        }
        target.push(InlineAnnotation::new(char_idx, label));
        if let Some(true) = hint.padding_right {
            padding_after_inlay_hints.push(InlineAnnotation::new(char_idx, " "));
        }
    }

    doc.set_inlay_hints(
        view_id,
        DocumentInlayHints {
            id: inlay_hints_id,
            type_inlay_hints,
            parameter_inlay_hints,
            other_inlay_hints,
            padding_before_inlay_hints,
            padding_after_inlay_hints,
        },
    );
    doc.inlay_hints_oudated = false;
}

pub(super) fn truncate_inlay_hint_label(label: &mut String, limit: Option<std::num::NonZeroU8>) {
    let Some(limit) = limit else {
        return;
    };

    use helix_core::unicode::{segmentation::UnicodeSegmentation, width::UnicodeWidthStr};

    let width = label.width();
    let limit = usize::from(limit.get());
    if width <= limit {
        return;
    }

    let mut truncate_at = 0;
    let mut acc = 0;
    for (index, grapheme_cluster) in label.grapheme_indices(true) {
        let grapheme_width = grapheme_cluster.width();
        if acc + grapheme_width > limit {
            break;
        }
        acc += grapheme_width;
        truncate_at = index + grapheme_cluster.len();
    }

    label.truncate(truncate_at);
    label.push('…');
}

impl Application {
    pub fn schedule_inlay_hints_for_visible_views(&mut self) {
        if !self.editor.config().lsp.display_inlay_hints {
            return;
        }

        let jobs = inlay_hint_jobs_for_visible_views(&self.editor);
        for job in jobs {
            self.jobs.callback(job);
        }
    }
}
