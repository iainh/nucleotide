//! Debounced pull diagnostics, cancellation, retries, and result application.

use std::collections::HashSet;
use std::time::Duration;

use futures_util::stream::{FuturesUnordered, StreamExt};
use helix_core::{Uri, syntax};
use helix_lsp::{LanguageServerId, lsp};
use helix_view::{Document, DocumentId, Editor, handlers::Handlers};
use nucleotide_logging::error;

#[derive(Debug, Default)]
pub(super) struct PullDiagnosticsHandler {
    document_ids: HashSet<DocumentId>,
}

impl helix_event::AsyncHook for PullDiagnosticsHandler {
    type Event = helix_view::handlers::lsp::PullDiagnosticsEvent;

    fn handle_event(
        &mut self,
        event: Self::Event,
        _timeout: Option<tokio::time::Instant>,
    ) -> Option<tokio::time::Instant> {
        self.document_ids.insert(event.document_id);
        Some(tokio::time::Instant::now() + Duration::from_millis(250))
    }

    fn finish_debounce(&mut self) {
        let document_ids = std::mem::take(&mut self.document_ids);
        helix_term::job::dispatch_blocking(move |editor, _| {
            for document_id in document_ids {
                request_document_diagnostics(editor, document_id);
            }
        });
    }
}

#[derive(Debug, Default)]
pub(super) struct PullAllDocumentsDiagnosticsHandler {
    language_servers: HashSet<LanguageServerId>,
}

impl helix_event::AsyncHook for PullAllDocumentsDiagnosticsHandler {
    type Event = helix_view::handlers::lsp::PullAllDocumentsDiagnosticsEvent;

    fn handle_event(
        &mut self,
        event: Self::Event,
        _timeout: Option<tokio::time::Instant>,
    ) -> Option<tokio::time::Instant> {
        self.language_servers.extend(event.language_servers);
        Some(tokio::time::Instant::now() + Duration::from_secs(1))
    }

    fn finish_debounce(&mut self) {
        let language_servers = std::mem::take(&mut self.language_servers);
        helix_term::job::dispatch_blocking(move |editor, _| {
            let document_ids: Vec<_> = editor.documents.keys().copied().collect();
            for document_id in document_ids {
                request_document_diagnostics_for_language_servers(
                    editor,
                    document_id,
                    language_servers.clone(),
                );
            }
        });
    }
}

pub(super) fn register_pull_diagnostic_hooks(handlers: &Handlers) {
    use helix_event::{register_hook, send_blocking};
    use helix_view::events::{DocumentDidChange, DocumentDidOpen, LanguageServerInitialized};
    use helix_view::handlers::lsp::{PullAllDocumentsDiagnosticsEvent, PullDiagnosticsEvent};

    let pull_diagnostics = handlers.pull_diagnostics.clone();
    let pull_all_documents_diagnostics = handlers.pull_all_documents_diagnostics.clone();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if event.ghost_transaction
            || !event.doc.has_language_server_with_feature(
                syntax::config::LanguageServerFeature::PullDiagnostics,
            )
        {
            return Ok(());
        }

        event.doc.pull_diagnostic_controller.cancel();
        send_blocking(
            &pull_diagnostics,
            PullDiagnosticsEvent {
                document_id: event.doc.id(),
            },
        );

        let language_servers = event
            .doc
            .language_servers_with_feature(syntax::config::LanguageServerFeature::PullDiagnostics)
            .filter(|language_server| {
                language_server
                    .capabilities()
                    .diagnostic_provider
                    .as_ref()
                    .is_some_and(|provider| match provider {
                        lsp::DiagnosticServerCapabilities::Options(options) => {
                            options.inter_file_dependencies
                        }
                        lsp::DiagnosticServerCapabilities::RegistrationOptions(options) => {
                            options.diagnostic_options.inter_file_dependencies
                        }
                    })
            })
            .map(|language_server| language_server.id())
            .collect();
        send_blocking(
            &pull_all_documents_diagnostics,
            PullAllDocumentsDiagnosticsEvent { language_servers },
        );
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        request_document_diagnostics(event.editor, event.doc);
        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        request_all_document_diagnostics_for_language_server(event.editor, event.server_id);
        Ok(())
    });
}

pub(super) fn request_all_document_diagnostics_for_language_server(
    editor: &mut Editor,
    server_id: LanguageServerId,
) {
    let document_ids: Vec<_> = editor
        .documents
        .values()
        .filter(|doc| doc.supports_language_server(server_id))
        .map(Document::id)
        .collect();

    for document_id in document_ids {
        request_document_diagnostics(editor, document_id);
    }
}

fn request_document_diagnostics(editor: &mut Editor, document_id: DocumentId) {
    let Some(doc) = editor.document(document_id) else {
        return;
    };

    let language_servers = doc
        .language_servers_with_feature(syntax::config::LanguageServerFeature::PullDiagnostics)
        .map(|language_server| language_server.id())
        .collect();

    request_document_diagnostics_for_language_servers(editor, document_id, language_servers);
}

fn request_document_diagnostics_for_language_servers(
    editor: &mut Editor,
    document_id: DocumentId,
    language_servers: HashSet<LanguageServerId>,
) {
    let Some(doc) = editor.document_mut(document_id) else {
        return;
    };

    let cancel = doc.pull_diagnostic_controller.restart();
    let mut futures: FuturesUnordered<_> = language_servers
        .iter()
        .filter_map(|server_id| {
            doc.language_servers()
                .find(|language_server| language_server.id() == *server_id)
        })
        .filter_map(|language_server| {
            let server_id = language_server.id();
            let request = language_server.text_document_diagnostic(
                doc.identifier(),
                doc.previous_diagnostic_ids.get(&server_id).cloned(),
            )?;
            let identifier = language_server
                .capabilities()
                .diagnostic_provider
                .as_ref()
                .and_then(|provider| match provider {
                    lsp::DiagnosticServerCapabilities::Options(options) => {
                        options.identifier.clone()
                    }
                    lsp::DiagnosticServerCapabilities::RegistrationOptions(options) => {
                        options.diagnostic_options.identifier.clone()
                    }
                });
            let provider = helix_core::diagnostic::DiagnosticProvider::Lsp {
                server_id,
                identifier,
            };
            let uri = doc.uri()?;

            Some(async move { (request.await, provider, uri) })
        })
        .collect();

    if futures.is_empty() {
        return;
    }

    tokio::spawn(async move {
        let mut retry_language_servers = HashSet::new();
        loop {
            match helix_event::cancelable_future(futures.next(), &cancel).await {
                Some(Some((Ok(result), provider, uri))) => {
                    helix_term::job::dispatch(move |editor, _| {
                        handle_pull_diagnostics_response(
                            editor,
                            result,
                            provider,
                            uri,
                            document_id,
                        );
                    })
                    .await;
                }
                Some(Some((Err(err), provider, _))) => {
                    let Some(server_id) = provider.language_server_id() else {
                        continue;
                    };
                    let cancellation_data = if let helix_lsp::Error::Rpc(error) = err {
                        error.data.and_then(|data| {
                            serde_json::from_value::<lsp::DiagnosticServerCancellationData>(data)
                                .ok()
                        })
                    } else {
                        error!(server_id = ?server_id, error = %err, "Pull diagnostic request failed");
                        continue;
                    };

                    if cancellation_data.is_some_and(|data| data.retrigger_request) {
                        retry_language_servers.insert(server_id);
                    }
                }
                Some(None) => break,
                None => return,
            }
        }

        if !retry_language_servers.is_empty() {
            tokio::time::sleep(Duration::from_millis(500)).await;
            helix_term::job::dispatch(move |editor, _| {
                request_document_diagnostics_for_language_servers(
                    editor,
                    document_id,
                    retry_language_servers,
                );
            })
            .await;
        }
    });
}

fn handle_pull_diagnostics_response(
    editor: &mut Editor,
    result: lsp::DocumentDiagnosticReportResult,
    provider: helix_core::diagnostic::DiagnosticProvider,
    uri: Uri,
    document_id: DocumentId,
) {
    let lsp::DocumentDiagnosticReportResult::Report(report) = result else {
        return;
    };

    let result_id = match report {
        lsp::DocumentDiagnosticReport::Full(report) => {
            editor.handle_lsp_diagnostics(
                &provider,
                uri,
                None,
                report.full_document_diagnostic_report.items,
            );
            report.full_document_diagnostic_report.result_id
        }
        lsp::DocumentDiagnosticReport::Unchanged(report) => {
            Some(report.unchanged_document_diagnostic_report.result_id)
        }
    };

    let Some(document) = editor.document_mut(document_id) else {
        return;
    };
    let Some(server_id) = provider.language_server_id() else {
        return;
    };

    match result_id {
        Some(result_id) => {
            document
                .previous_diagnostic_ids
                .insert(server_id, result_id);
        }
        None => {
            document.previous_diagnostic_ids.remove(&server_id);
        }
    }
}
