// ABOUTME: LSP (Language Server Protocol) management functionality
// ABOUTME: Handles language server messages and progress tracking

use helix_core::diagnostic::{DiagnosticProvider, Severity};
use helix_lsp::{
    lsp::{self},
    Call, LanguageServerId, LspProgressMap, MethodCall, Notification,
};
use helix_view::Editor;
use nucleotide_logging::{error, info, instrument, timed, warn};
use std::collections::btree_map::Entry;

/// Manages LSP operations and message handling
pub struct LspManager<'a> {
    editor: &'a mut Editor,
    lsp_progress: &'a mut LspProgressMap,
}

impl<'a> LspManager<'a> {
    pub fn new(editor: &'a mut Editor, lsp_progress: &'a mut LspProgressMap) -> Self {
        Self {
            editor,
            lsp_progress,
        }
    }

    /// Handle a language server message
    #[instrument(skip(self))]
    pub async fn handle_language_server_message(
        &mut self,
        call: Call,
        server_id: LanguageServerId,
    ) {
        let _timer = timed!("lsp_message_handling", warn_threshold: std::time::Duration::from_millis(100), {
        // Track that we've seen this server
        // Note: We'll need to pass the LspState entity down from Application
        // For now, just handle the messages

        match call {
            Call::Notification(helix_lsp::jsonrpc::Notification { method, params, .. }) => {
                let notification = match Notification::parse(&method, params) {
                    Ok(notification) => notification,
                    Err(helix_lsp::Error::Unhandled) => {
                        info!("Ignoring Unhandled notification from Language Server");
                        return;
                    }
                    Err(err) => {
                        error!(
                            error = %err,
                            "Ignoring unknown notification from Language Server"
                        );
                        return;
                    }
                };

                match notification {
                    Notification::PublishDiagnostics(params) => {
                        self.handle_diagnostics_published(params, server_id);
                    }
                    Notification::ProgressMessage(params) => {
                        self.handle_progress_notification(params, server_id);
                    }
                    Notification::LogMessage(params) => {
                        info!(message = %params.message, "LSP Log");
                    }
                    Notification::ShowMessage(params) => {
                        let _severity = match params.typ {
                            helix_lsp::lsp::MessageType::ERROR => Severity::Error,
                            helix_lsp::lsp::MessageType::WARNING => Severity::Warning,
                            helix_lsp::lsp::MessageType::INFO => Severity::Info,
                            helix_lsp::lsp::MessageType::LOG => Severity::Hint,
                            _ => Severity::Info,
                        };
                        self.editor.set_status(params.message);
                    }
                    Notification::Initialized => {
                        self.handle_initialized_notification(server_id);
                    }
                    Notification::Exit => {
                        self.handle_exit_notification(server_id);
                    }
                }
            }
            Call::MethodCall(method_call) => {
                self.handle_method_call(method_call, server_id).await;
            }
            Call::Invalid { id } => {
                error!(id = ?id, "LSP invalid method call");
            }
        }
        }); // Close the timed block
    }

    #[instrument(skip(self), fields(uri = %params.uri))]
    fn handle_diagnostics_published(
        &mut self,
        mut params: helix_lsp::lsp::PublishDiagnosticsParams,
        server_id: LanguageServerId,
    ) {
        let path = match params.uri.to_file_path() {
            Ok(path) => helix_stdx::path::normalize(path),
            Err(_) => {
                error!(uri = %params.uri, "Unsupported file URI");
                return;
            }
        };

        let language_server = match self.editor.language_server_by_id(server_id) {
            Some(ls) => ls,
            None => {
                warn!(server_id = ?server_id, "Can't find language server with id");
                return;
            }
        };

        if !language_server.is_initialized() {
            error!(
                server_name = %language_server.name(),
                "Discarding publishDiagnostic notification sent by an uninitialized server"
            );
            return;
        }

        // Find the document with this path
        let doc = self
            .editor
            .documents
            .values_mut()
            .find(|doc| doc.path().map(|p| p == &path).unwrap_or(false))
            .filter(|doc| {
                if let Some(version) = params.version {
                    if version != doc.version() {
                        info!(
                            version = version,
                            path = ?path,
                            expected_version = doc.version(),
                            "Version is out of date, dropping PublishDiagnostic notification"
                        );
                        return false;
                    }
                }
                true
            });

        let mut unchanged_diag_sources = Vec::new();
        if let Some(doc) = &doc {
            let lang_conf = doc.language.clone();

            if let Some(lang_conf) = &lang_conf {
                let uri = helix_core::Uri::from(path.clone());
                if let Some(old_diagnostics) = self.editor.diagnostics.get(&uri) {
                    if !lang_conf.persistent_diagnostic_sources.is_empty() {
                        // Sort diagnostics first by severity and then by line numbers.
                        params
                            .diagnostics
                            .sort_unstable_by_key(|d| (d.severity, d.range.start));
                    }
                    for source in &lang_conf.persistent_diagnostic_sources {
                        let new_diagnostics = params
                            .diagnostics
                            .iter()
                            .filter(|d| d.source.as_ref() == Some(source));
                        let old_diagnostics = old_diagnostics
                            .iter()
                            .filter(|(d, d_server)| {
                                d_server.language_server_id() == Some(server_id)
                                    && d.source.as_ref() == Some(source)
                            })
                            .map(|(d, _)| d);
                        if new_diagnostics.eq(old_diagnostics) {
                            unchanged_diag_sources.push(source.clone())
                        }
                    }
                }
            }
        }

        let provider = DiagnosticProvider::Lsp {
            server_id,
            identifier: None,
        };
        let diagnostics = params
            .diagnostics
            .into_iter()
            .map(|d| (d, provider.clone()));

        // Insert the diagnostics
        let uri = helix_core::Uri::from(path.clone());
        let diagnostics = match self.editor.diagnostics.entry(uri) {
            Entry::Occupied(o) => {
                let current_diagnostics = o.into_mut();
                // there may be entries of other language servers, which is why we can't overwrite the whole entry
                current_diagnostics
                    .retain(|(_, provider)| provider.language_server_id() != Some(server_id));
                current_diagnostics.extend(diagnostics);
                current_diagnostics
            }
            Entry::Vacant(v) => v.insert(diagnostics.collect()),
        };

        // Sort diagnostics first by severity and then by line numbers.
        diagnostics
            .sort_unstable_by_key(|(d, server_id)| (d.severity, d.range.start, server_id.clone()));

        if let Some(doc) = doc {
            let diagnostic_of_language_server_and_not_in_unchanged_sources =
                |diagnostic: &lsp::Diagnostic, provider: &DiagnosticProvider| {
                    provider.language_server_id() == Some(server_id)
                        && diagnostic
                            .source
                            .as_ref()
                            .is_none_or(|source| !unchanged_diag_sources.contains(source))
                };
            let diagnostics = Editor::doc_diagnostics_with_filter(
                &self.editor.language_servers,
                &self.editor.diagnostics,
                doc,
                diagnostic_of_language_server_and_not_in_unchanged_sources,
            );
            doc.replace_diagnostics(
                diagnostics,
                &unchanged_diag_sources,
                Some(&DiagnosticProvider::Lsp {
                    server_id,
                    identifier: None,
                }),
            );
        }
    }

    fn handle_progress_notification(
        &mut self,
        params: helix_lsp::lsp::ProgressParams,
        server_id: LanguageServerId,
    ) {
        use helix_lsp::lsp::ProgressParamsValue;

        let token = params.token.clone();

        match params.value {
            ProgressParamsValue::WorkDone(progress) => {
                use helix_lsp::lsp::WorkDoneProgress;

                match progress {
                    WorkDoneProgress::Begin(begin) => {
                        self.lsp_progress.create(server_id, token.clone());

                        if let Some(message) = begin.message {
                            self.editor.set_status(message);
                        }
                    }
                    WorkDoneProgress::Report(report) => {
                        if let Some(message) = report.message {
                            self.editor.set_status(message);
                        }
                    }
                    WorkDoneProgress::End(end) => {
                        self.lsp_progress.end_progress(server_id, &token);

                        if let Some(message) = end.message {
                            self.editor.set_status(message);
                        }
                    }
                }
            }
        }
    }

    #[instrument(skip(self), fields(method = %method_call.method))]
    async fn handle_method_call(
        &mut self,
        method_call: helix_lsp::jsonrpc::MethodCall,
        server_id: LanguageServerId,
    ) {
        use helix_lsp::lsp;

        // First check if language server exists
        let (is_initialized, offset_encoding) = match self.editor.language_server_by_id(server_id) {
            Some(ls) => (ls.is_initialized(), ls.offset_encoding()),
            None => return,
        };

        let reply = match MethodCall::parse(&method_call.method, method_call.params) {
            Err(helix_lsp::Error::Unhandled) => {
                error!(
                    method = %method_call.method,
                    request_id = %method_call.id,
                    "Language Server: Method not found in request"
                );
                Err(helix_lsp::jsonrpc::Error {
                    code: helix_lsp::jsonrpc::ErrorCode::MethodNotFound,
                    message: format!("Method not found: {}", method_call.method),
                    data: None,
                })
            }
            Err(err) => {
                error!(
                    method = %method_call.method,
                    request_id = %method_call.id,
                    error = %err,
                    "Language Server: Received malformed method call in request"
                );
                Err(helix_lsp::jsonrpc::Error {
                    code: helix_lsp::jsonrpc::ErrorCode::ParseError,
                    message: format!("Malformed method call: {}", method_call.method),
                    data: None,
                })
            }
            Ok(MethodCall::WorkDoneProgressCreate(params)) => {
                self.lsp_progress.create(server_id, params.token);
                Ok(serde_json::Value::Null)
            }
            Ok(MethodCall::ApplyWorkspaceEdit(params)) => {
                if is_initialized {
                    let res = self
                        .editor
                        .apply_workspace_edit(offset_encoding, &params.edit);

                    Ok(serde_json::json!(lsp::ApplyWorkspaceEditResponse {
                        applied: res.is_ok(),
                        failure_reason: res.as_ref().err().map(|err| err.kind.to_string()),
                        failed_change: res.as_ref().err().map(|err| err.failed_change_idx as u32),
                    }))
                } else {
                    Err(helix_lsp::jsonrpc::Error {
                        code: helix_lsp::jsonrpc::ErrorCode::InvalidRequest,
                        message: "Server must be initialized to request workspace edits"
                            .to_string(),
                        data: None,
                    })
                }
            }
            Ok(MethodCall::WorkspaceFolders) => {
                // Get language server again for this specific operation
                match self.editor.language_server_by_id(server_id) {
                    Some(ls) => Ok(serde_json::json!(&*ls.workspace_folders().await)),
                    None => Err(helix_lsp::jsonrpc::Error {
                        code: helix_lsp::jsonrpc::ErrorCode::InternalError,
                        message: "Language server not found".to_string(),
                        data: None,
                    }),
                }
            }
            Ok(MethodCall::WorkspaceConfiguration(params)) => {
                // Get language server again for config access
                match self.editor.language_server_by_id(server_id) {
                    Some(ls) => {
                        let result: Vec<_> = params
                            .items
                            .iter()
                            .map(|item| {
                                let mut config = ls.config()?;
                                if let Some(section) = item.section.as_ref() {
                                    // for some reason some lsps send an empty string (observed in 'vscode-eslint-language-server')
                                    if !section.is_empty() {
                                        for part in section.split('.') {
                                            config = config.get(part)?;
                                        }
                                    }
                                }
                                Some(config)
                            })
                            .collect();
                        Ok(serde_json::json!(result))
                    }
                    None => Err(helix_lsp::jsonrpc::Error {
                        code: helix_lsp::jsonrpc::ErrorCode::InternalError,
                        message: "Language server not found".to_string(),
                        data: None,
                    }),
                }
            }
            Ok(MethodCall::RegisterCapability(params)) => {
                if let Some(client) = self.editor.language_servers.get_by_id(server_id) {
                    for reg in params.registrations {
                        match reg.method.as_str() {
                            "workspace/didChangeWatchedFiles" => {
                                let Some(options) = reg.register_options else {
                                    continue;
                                };
                                let ops: lsp::DidChangeWatchedFilesRegistrationOptions =
                                    match serde_json::from_value(options) {
                                        Ok(ops) => ops,
                                        Err(err) => {
                                            warn!(error = %err, "Failed to deserialize DidChangeWatchedFilesRegistrationOptions");
                                            continue;
                                        }
                                    };
                                self.editor.language_servers.file_event_handler.register(
                                    client.id(),
                                    std::sync::Arc::downgrade(client),
                                    reg.id,
                                    ops,
                                )
                            }
                            _ => {
                                // Language Servers based on the `vscode-languageserver-node` library often send
                                // client/registerCapability even though we do not enable dynamic registration
                                // for most capabilities. We should send a MethodNotFound JSONRPC error in this
                                // case but that rejects the registration promise in the server which causes an
                                // exit. So we work around this by ignoring the request and sending back an OK
                                // response.
                                warn!("Ignoring a client/registerCapability request because dynamic capability registration is not enabled. Please report this upstream to the language server");
                            }
                        }
                    }
                }
                Ok(serde_json::Value::Null)
            }
            Ok(MethodCall::UnregisterCapability(params)) => {
                for unreg in params.unregisterations {
                    match unreg.method.as_str() {
                        "workspace/didChangeWatchedFiles" => {
                            self.editor
                                .language_servers
                                .file_event_handler
                                .unregister(server_id, unreg.id);
                        }
                        _ => {
                            warn!(
                                method = %unreg.method,
                                "Received unregistration request for unsupported method"
                            );
                        }
                    }
                }
                Ok(serde_json::Value::Null)
            }
            Ok(MethodCall::ShowDocument(_params)) => {
                // For now, just return success
                let result = lsp::ShowDocumentResult { success: true };
                Ok(serde_json::json!(result))
            }
        };

        // Get language server again to send reply
        if let Some(language_server) = self.editor.language_server_by_id(server_id) {
            if let Err(err) = language_server.reply(method_call.id, reply) {
                error!(error = %err, "Failed to reply to method call");
            }
        }
    }

    fn handle_initialized_notification(&mut self, server_id: LanguageServerId) {
        let language_server = match self.editor.language_server_by_id(server_id) {
            Some(ls) => ls,
            None => return,
        };

        // Trigger a workspace/didChangeConfiguration notification after initialization.
        // This might not be required by the spec but Neovim does this as well, so it's
        // probably a good idea for compatibility.
        if let Some(config) = language_server.config() {
            language_server.did_change_configuration(config.clone());
        }

        let docs = self
            .editor
            .documents()
            .filter(|doc| doc.supports_language_server(server_id));

        // trigger textDocument/didOpen for docs that are already open
        for doc in docs {
            let url = match doc.url() {
                Some(url) => url,
                None => continue, // skip documents with no path
            };

            let language_id = doc.language_id().map(ToOwned::to_owned).unwrap_or_default();

            language_server.text_document_did_open(url, doc.version(), doc.text(), language_id);
        }
    }

    fn handle_exit_notification(&mut self, server_id: LanguageServerId) {
        self.editor.set_status("Language server exited");

        // LSPs may produce diagnostics for files that haven't been opened in helix,
        // we need to clear those and remove the entries from the list if this leads to
        // an empty diagnostic list for said files
        for diags in self.editor.diagnostics.values_mut() {
            diags.retain(|(_, provider)| provider.language_server_id() != Some(server_id));
        }

        self.editor.diagnostics.retain(|_, diags| !diags.is_empty());

        // Clear any diagnostics for documents with this server open.
        for doc in self.editor.documents_mut() {
            doc.clear_diagnostics_for_language_server(server_id);
        }

        // Remove the language server from the registry.
        self.editor.language_servers.remove_by_id(server_id);
    }

    // Removed get_progress_items - progress is now handled by events and LspState
}
