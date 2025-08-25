// ABOUTME: Completion generation engine that creates mock completion responses based on context and configuration
// ABOUTME: Supports different completion types, test scenarios, and language-agnostic completion generation

use crate::config::{CompletionTemplate, TestLspConfig};
use anyhow::Result;
use lsp_types::*;
use std::time::Duration;
use tracing::{debug, info};

pub struct CompletionEngine {
    config: TestLspConfig,
}

impl CompletionEngine {
    pub fn new(config: TestLspConfig) -> Self {
        Self { config }
    }

    /// Generate completion items based on the request context
    pub fn generate_completions(&self, params: &CompletionParams) -> Result<Vec<CompletionItem>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = &params.text_document_position.position;

        info!(
            "Generating completions for {:?} at {}:{}",
            uri, position.line, position.character
        );

        // Extract file extension or use context to determine scenario
        let file_extension = self.extract_file_extension(uri);
        let scenario = self.config.get_scenario_for_context(&file_extension);

        debug!("Using test scenario: {}", scenario.name);

        // Handle test scenario delays
        if let Some(delay_ms) = scenario.delay_ms {
            info!("Simulating {}ms delay", delay_ms);
            std::thread::sleep(Duration::from_millis(delay_ms));
        }

        // Handle failure scenarios
        if scenario.should_fail {
            info!(
                "Simulating LSP server error for scenario: {}",
                scenario.name
            );
            return Err(anyhow::anyhow!(
                "Simulated LSP server error: {}",
                scenario.error_message.as_deref().unwrap_or("Unknown error")
            ));
        }

        // Generate completions based on scenario
        let completion_count = scenario.completion_count.unwrap_or(5);
        let completions = if let Some(custom_completions) = &scenario.custom_completions {
            self.generate_custom_completions(custom_completions, completion_count)
        } else {
            self.generate_default_completions(completion_count, &file_extension, position)
        };

        info!("Generated {} completion items", completions.len());
        Ok(completions)
    }

    /// Extract file extension from URI
    fn extract_file_extension(&self, uri: &Uri) -> String {
        uri.path()
            .as_str()
            .split('.')
            .last()
            .unwrap_or("txt")
            .to_string()
    }

    /// Generate custom completions from templates
    fn generate_custom_completions(
        &self,
        templates: &[CompletionTemplate],
        max_count: usize,
    ) -> Vec<CompletionItem> {
        templates
            .iter()
            .take(max_count)
            .enumerate()
            .map(|(index, template)| self.template_to_completion_item(template, index))
            .collect()
    }

    /// Generate default completions using built-in templates
    fn generate_default_completions(
        &self,
        count: usize,
        file_extension: &str,
        position: &Position,
    ) -> Vec<CompletionItem> {
        if count == 0 {
            return Vec::new();
        }

        let mut completions = Vec::new();
        let templates = &self.config.completion.templates;

        // Generate completions based on context
        let completion_types = self.determine_completion_types(file_extension, position);

        for (index, completion_type) in completion_types.iter().enumerate() {
            if index >= count {
                break;
            }

            if let Some(template) = templates.get(completion_type) {
                completions.push(self.template_to_completion_item(template, index));
            } else {
                // Fallback to generated completion
                completions.push(self.generate_fallback_completion(completion_type, index));
            }
        }

        // If we need more completions, generate numbered variants
        while completions.len() < count {
            let index = completions.len();
            completions.push(self.generate_numbered_completion(index, file_extension));
        }

        completions
    }

    /// Determine appropriate completion types based on context
    fn determine_completion_types(
        &self,
        file_extension: &str,
        _position: &Position,
    ) -> Vec<String> {
        match file_extension {
            "rs" => vec![
                "function".to_string(),
                "method".to_string(),
                "variable".to_string(),
                "keyword".to_string(),
                "snippet".to_string(),
            ],
            "js" | "ts" => vec![
                "function".to_string(),
                "method".to_string(),
                "variable".to_string(),
                "keyword".to_string(),
            ],
            "py" => vec![
                "function".to_string(),
                "method".to_string(),
                "variable".to_string(),
                "keyword".to_string(),
            ],
            "go" => vec![
                "function".to_string(),
                "method".to_string(),
                "variable".to_string(),
                "keyword".to_string(),
            ],
            _ => vec![
                "function".to_string(),
                "variable".to_string(),
                "keyword".to_string(),
            ],
        }
    }

    /// Convert a completion template to LSP CompletionItem
    fn template_to_completion_item(
        &self,
        template: &CompletionTemplate,
        index: usize,
    ) -> CompletionItem {
        CompletionItem {
            label: template.label.clone(),
            label_details: None,
            kind: Some(self.string_to_completion_item_kind(&template.kind)),
            detail: template.detail.clone(),
            documentation: template.documentation.as_ref().map(|doc| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: doc.clone(),
                })
            }),
            deprecated: Some(false),
            preselect: Some(index == 0),
            sort_text: template
                .sort_text
                .clone()
                .or_else(|| Some(format!("{:04}", index))),
            filter_text: template.filter_text.clone(),
            insert_text: template.insert_text.clone(),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            insert_text_mode: None,
            text_edit: None,
            additional_text_edits: None,
            command: None,
            commit_characters: None,
            data: None,
            tags: None,
        }
    }

    /// Generate a fallback completion when template is not available
    fn generate_fallback_completion(&self, completion_type: &str, index: usize) -> CompletionItem {
        CompletionItem {
            label: format!("test_{}", completion_type),
            label_details: None,
            kind: Some(self.string_to_completion_item_kind(completion_type)),
            detail: Some(format!("Test {} completion", completion_type)),
            documentation: Some(Documentation::String(format!(
                "Generated test completion of type: {}",
                completion_type
            ))),
            deprecated: Some(false),
            preselect: Some(index == 0),
            sort_text: Some(format!("{:04}", index)),
            filter_text: None,
            insert_text: Some(format!("test_{}", completion_type)),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            insert_text_mode: None,
            text_edit: None,
            additional_text_edits: None,
            command: None,
            commit_characters: None,
            data: None,
            tags: None,
        }
    }

    /// Generate numbered completion items for padding
    fn generate_numbered_completion(&self, index: usize, file_extension: &str) -> CompletionItem {
        CompletionItem {
            label: format!("test_completion_{}", index),
            label_details: None,
            kind: Some(CompletionItemKind::TEXT),
            detail: Some(format!("Test completion #{} for {}", index, file_extension)),
            documentation: Some(Documentation::String(format!(
                "Generated test completion item #{} for file type: {}",
                index, file_extension
            ))),
            deprecated: Some(false),
            preselect: Some(false),
            sort_text: Some(format!("{:04}", index)),
            filter_text: None,
            insert_text: Some(format!("test_completion_{}", index)),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            insert_text_mode: None,
            text_edit: None,
            additional_text_edits: None,
            command: None,
            commit_characters: None,
            data: None,
            tags: None,
        }
    }

    /// Convert string to CompletionItemKind
    fn string_to_completion_item_kind(&self, kind_str: &str) -> CompletionItemKind {
        match kind_str.to_lowercase().as_str() {
            "text" => CompletionItemKind::TEXT,
            "method" => CompletionItemKind::METHOD,
            "function" => CompletionItemKind::FUNCTION,
            "constructor" => CompletionItemKind::CONSTRUCTOR,
            "field" => CompletionItemKind::FIELD,
            "variable" => CompletionItemKind::VARIABLE,
            "class" => CompletionItemKind::CLASS,
            "interface" => CompletionItemKind::INTERFACE,
            "module" => CompletionItemKind::MODULE,
            "property" => CompletionItemKind::PROPERTY,
            "unit" => CompletionItemKind::UNIT,
            "value" => CompletionItemKind::VALUE,
            "enum" => CompletionItemKind::ENUM,
            "keyword" => CompletionItemKind::KEYWORD,
            "snippet" => CompletionItemKind::SNIPPET,
            "color" => CompletionItemKind::COLOR,
            "file" => CompletionItemKind::FILE,
            "reference" => CompletionItemKind::REFERENCE,
            "folder" => CompletionItemKind::FOLDER,
            "enummember" => CompletionItemKind::ENUM_MEMBER,
            "constant" => CompletionItemKind::CONSTANT,
            "struct" => CompletionItemKind::STRUCT,
            "event" => CompletionItemKind::EVENT,
            "operator" => CompletionItemKind::OPERATOR,
            "typeparameter" => CompletionItemKind::TYPE_PARAMETER,
            _ => CompletionItemKind::TEXT,
        }
    }
}
