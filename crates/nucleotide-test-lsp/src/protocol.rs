// ABOUTME: LSP protocol handler that manages document state and protocol-specific operations
// ABOUTME: Tracks document changes and provides context for completion generation

use crate::config::TestLspConfig;
use anyhow::Result;
use lsp_types::*;
use std::collections::HashMap;
use tracing::{debug, info};

pub struct ProtocolHandler {
    config: TestLspConfig,
    documents: HashMap<Uri, DocumentState>,
}

#[derive(Debug, Clone)]
pub struct DocumentState {
    pub uri: Uri,
    pub language_id: String,
    pub version: i32,
    pub content: String,
}

impl ProtocolHandler {
    pub fn new(config: TestLspConfig) -> Self {
        Self {
            config,
            documents: HashMap::new(),
        }
    }

    /// Handle document open notification
    pub fn handle_did_open(&mut self, params: &DidOpenTextDocumentParams) -> Result<()> {
        let document = &params.text_document;

        info!(
            "Opening document: {:?} ({})",
            document.uri, document.language_id
        );

        let state = DocumentState {
            uri: document.uri.clone(),
            language_id: document.language_id.clone(),
            version: document.version,
            content: document.text.clone(),
        };

        self.documents.insert(document.uri.clone(), state);
        Ok(())
    }

    /// Handle document change notification
    pub fn handle_did_change(&mut self, params: &DidChangeTextDocumentParams) -> Result<()> {
        let document = &params.text_document;

        debug!(
            "Document changed: {:?} (version {})",
            document.uri, document.version
        );

        if let Some(state) = self.documents.get_mut(&document.uri) {
            state.version = document.version;

            // Apply changes to document content
            for change in &params.content_changes {
                match change.range {
                    Some(range) => {
                        // Incremental change
                        Self::apply_incremental_change(state, &range, &change.text)?;
                    }
                    None => {
                        // Full document change
                        state.content = change.text.clone();
                    }
                }
            }
        } else {
            debug!("Received change for unknown document: {:?}", document.uri);
        }

        Ok(())
    }

    /// Handle document save notification
    pub fn handle_did_save(&mut self, params: &DidSaveTextDocumentParams) -> Result<()> {
        info!("Document saved: {:?}", params.text_document.uri);

        // Update content if provided
        if let Some(text) = &params.text {
            if let Some(state) = self.documents.get_mut(&params.text_document.uri) {
                state.content = text.clone();
            }
        }

        Ok(())
    }

    /// Handle document close notification
    pub fn handle_did_close(&mut self, params: &DidCloseTextDocumentParams) -> Result<()> {
        info!("Document closed: {:?}", params.text_document.uri);
        self.documents.remove(&params.text_document.uri);
        Ok(())
    }

    /// Get document state by URI
    pub fn get_document(&self, uri: &Uri) -> Option<&DocumentState> {
        self.documents.get(uri)
    }

    /// Get text around a position for context-aware completion
    pub fn get_context_at_position(&self, uri: &Uri, position: &Position) -> Option<String> {
        let state = self.documents.get(uri)?;
        let lines: Vec<&str> = state.content.lines().collect();

        if (position.line as usize) >= lines.len() {
            return None;
        }

        let line = lines[position.line as usize];
        let char_pos = position.character as usize;

        if char_pos > line.len() {
            return None;
        }

        // Get text before cursor on current line
        let prefix = &line[..char_pos.min(line.len())];

        // For context, also include previous lines
        let start_line = if position.line >= 3 {
            position.line - 3
        } else {
            0
        };

        let context_lines = &lines[start_line as usize..=position.line as usize];
        let mut context = context_lines[..context_lines.len().saturating_sub(1)].join("\n");

        if !context.is_empty() {
            context.push('\n');
        }
        context.push_str(prefix);

        Some(context)
    }

    /// Apply incremental change to document content
    fn apply_incremental_change(
        state: &mut DocumentState,
        range: &Range,
        new_text: &str,
    ) -> Result<()> {
        let lines: Vec<&str> = state.content.lines().collect();
        let mut result = Vec::new();

        // Add lines before the change
        for (i, &line) in lines.iter().enumerate() {
            if i < range.start.line as usize {
                result.push(line.to_string());
            } else if i == range.start.line as usize {
                // Handle the start line of the change
                let start_char = range.start.character as usize;
                let line_prefix = if start_char <= line.len() {
                    &line[..start_char]
                } else {
                    line
                };

                if range.start.line == range.end.line {
                    // Single line change
                    let end_char = range.end.character as usize;
                    let line_suffix = if end_char <= line.len() {
                        &line[end_char..]
                    } else {
                        ""
                    };

                    result.push(format!("{}{}{}", line_prefix, new_text, line_suffix));
                } else {
                    // Multi-line change - add prefix + new text
                    result.push(format!("{}{}", line_prefix, new_text));

                    // Find the end line and add its suffix
                    if let Some(&end_line) = lines.get(range.end.line as usize) {
                        let end_char = range.end.character as usize;
                        let line_suffix = if end_char <= end_line.len() {
                            &end_line[end_char..]
                        } else {
                            ""
                        };
                        if !line_suffix.is_empty() {
                            if let Some(last) = result.last_mut() {
                                last.push_str(line_suffix);
                            }
                        }
                    }
                }
                break;
            }
        }

        // Add lines after the change
        for (i, &line) in lines.iter().enumerate() {
            if i > range.end.line as usize {
                result.push(line.to_string());
            }
        }

        state.content = result.join("\n");
        Ok(())
    }

    /// Get the file extension for a URI
    pub fn get_file_extension(&self, uri: &Uri) -> String {
        uri.path()
            .as_str()
            .split('.')
            .last()
            .unwrap_or("txt")
            .to_string()
    }

    /// Get the language identifier for a document
    pub fn get_language_id(&self, uri: &Uri) -> Option<String> {
        self.documents
            .get(uri)
            .map(|state| state.language_id.clone())
    }
}
