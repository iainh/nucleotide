// ABOUTME: Configuration management for the test LSP server, handles loading completion templates and test scenarios
// ABOUTME: Supports TOML-based configuration with hot-reloading capabilities for development iteration

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestLspConfig {
    pub server: ServerConfig,
    pub completion: CompletionConfig,
    pub test_scenarios: HashMap<String, TestScenario>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    pub version: String,
    pub response_delay_ms: Option<u64>,
    pub max_completions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionConfig {
    pub templates: HashMap<String, CompletionTemplate>,
    pub context_aware: bool,
    pub fuzzy_matching: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionTemplate {
    pub label: String,
    pub kind: String,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub insert_text: Option<String>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,
    pub triggers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestScenario {
    pub name: String,
    pub description: String,
    pub delay_ms: Option<u64>,
    pub should_fail: bool,
    pub error_message: Option<String>,
    pub completion_count: Option<usize>,
    pub custom_completions: Option<Vec<CompletionTemplate>>,
}

impl TestLspConfig {
    /// Load the default configuration with sensible defaults
    pub fn load_default() -> Result<Self> {
        Ok(Self {
            server: ServerConfig {
                name: "nucleotide-test-lsp".to_string(),
                version: "0.1.0".to_string(),
                response_delay_ms: None,
                max_completions: 50,
            },
            completion: CompletionConfig {
                templates: Self::default_completion_templates(),
                context_aware: true,
                fuzzy_matching: true,
            },
            test_scenarios: Self::default_test_scenarios(),
        })
    }

    /// Load configuration from a file path
    pub fn load_from_file(path: PathBuf) -> Result<Self> {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {:?}", path))?;

        let config: Self = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {:?}", path))?;

        Ok(config)
    }

    /// Default completion templates that work across languages
    fn default_completion_templates() -> HashMap<String, CompletionTemplate> {
        let mut templates = HashMap::new();

        // Generic function completion
        templates.insert(
            "function".to_string(),
            CompletionTemplate {
                label: "test_function".to_string(),
                kind: "Function".to_string(),
                detail: Some("fn test_function() -> ()".to_string()),
                documentation: Some("A test function for completion testing".to_string()),
                insert_text: Some("test_function()".to_string()),
                filter_text: Some("test_function".to_string()),
                sort_text: Some("0001".to_string()),
                triggers: vec![".".to_string(), " ".to_string()],
            },
        );

        // Generic variable completion
        templates.insert(
            "variable".to_string(),
            CompletionTemplate {
                label: "test_variable".to_string(),
                kind: "Variable".to_string(),
                detail: Some("test_variable: String".to_string()),
                documentation: Some("A test variable for completion testing".to_string()),
                insert_text: Some("test_variable".to_string()),
                filter_text: Some("test_variable".to_string()),
                sort_text: Some("0002".to_string()),
                triggers: vec![".".to_string(), " ".to_string()],
            },
        );

        // Generic method completion
        templates.insert(
            "method".to_string(),
            CompletionTemplate {
                label: "test_method".to_string(),
                kind: "Method".to_string(),
                detail: Some("test_method(&self) -> String".to_string()),
                documentation: Some("A test method for completion testing".to_string()),
                insert_text: Some("test_method()".to_string()),
                filter_text: Some("test_method".to_string()),
                sort_text: Some("0003".to_string()),
                triggers: vec![".".to_string()],
            },
        );

        // Generic keyword completion
        templates.insert(
            "keyword".to_string(),
            CompletionTemplate {
                label: "test_keyword".to_string(),
                kind: "Keyword".to_string(),
                detail: Some("test_keyword".to_string()),
                documentation: Some("A test keyword for completion testing".to_string()),
                insert_text: Some("test_keyword".to_string()),
                filter_text: Some("test_keyword".to_string()),
                sort_text: Some("0004".to_string()),
                triggers: vec![" ".to_string()],
            },
        );

        // Generic snippet completion
        templates.insert(
            "snippet".to_string(),
            CompletionTemplate {
                label: "test_snippet".to_string(),
                kind: "Snippet".to_string(),
                detail: Some("Test snippet".to_string()),
                documentation: Some("A test snippet with placeholders".to_string()),
                insert_text: Some("test_snippet(${1:arg1}, ${2:arg2})$0".to_string()),
                filter_text: Some("test_snippet".to_string()),
                sort_text: Some("0005".to_string()),
                triggers: vec![" ".to_string()],
            },
        );

        templates
    }

    /// Default test scenarios for various testing needs
    fn default_test_scenarios() -> HashMap<String, TestScenario> {
        let mut scenarios = HashMap::new();

        scenarios.insert(
            "normal".to_string(),
            TestScenario {
                name: "Normal Response".to_string(),
                description: "Standard completion response with normal timing".to_string(),
                delay_ms: None,
                should_fail: false,
                error_message: None,
                completion_count: Some(5),
                custom_completions: None,
            },
        );

        scenarios.insert(
            "slow".to_string(),
            TestScenario {
                name: "Slow Response".to_string(),
                description: "Delayed completion response for testing timeout handling".to_string(),
                delay_ms: Some(2000),
                should_fail: false,
                error_message: None,
                completion_count: Some(3),
                custom_completions: None,
            },
        );

        scenarios.insert(
            "large".to_string(),
            TestScenario {
                name: "Large Result Set".to_string(),
                description: "Large number of completions for testing UI performance".to_string(),
                delay_ms: None,
                should_fail: false,
                error_message: None,
                completion_count: Some(100),
                custom_completions: None,
            },
        );

        scenarios.insert(
            "failure".to_string(),
            TestScenario {
                name: "Server Error".to_string(),
                description: "Simulated server error for testing error handling".to_string(),
                delay_ms: None,
                should_fail: true,
                error_message: Some("Test LSP server error".to_string()),
                completion_count: None,
                custom_completions: None,
            },
        );

        scenarios.insert(
            "empty".to_string(),
            TestScenario {
                name: "Empty Response".to_string(),
                description: "No completions available".to_string(),
                delay_ms: None,
                should_fail: false,
                error_message: None,
                completion_count: Some(0),
                custom_completions: None,
            },
        );

        scenarios
    }

    /// Get the active test scenario based on file type or context
    pub fn get_scenario_for_context(&self, file_extension: &str) -> &TestScenario {
        // For now, use file extension to determine scenario
        let scenario_name = match file_extension {
            "slow" => "slow",
            "large" => "large",
            "error" => "failure",
            "empty" => "empty",
            _ => "normal",
        };

        self.test_scenarios
            .get(scenario_name)
            .unwrap_or_else(|| &self.test_scenarios["normal"])
    }
}
