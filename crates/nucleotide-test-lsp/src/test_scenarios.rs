// ABOUTME: Test scenario definitions and utilities for simulating different LSP server behaviors
// ABOUTME: Provides scenarios for testing timeout handling, error conditions, and performance characteristics

use crate::config::{CompletionTemplate, TestScenario};
use lsp_types::*;
use std::collections::HashMap;

/// Scenario selector that determines which test scenario to use based on context
pub struct ScenarioSelector;

impl ScenarioSelector {
    /// Select a scenario based on file path, extension, or other context
    pub fn select_scenario<'a>(
        uri: &Uri,
        scenarios: &'a HashMap<String, TestScenario>,
    ) -> Option<&'a TestScenario> {
        let path = uri.path().as_str();

        // Check for scenario markers in file path
        if path.contains("slow") || path.contains("delay") {
            return scenarios.get("slow");
        }

        if path.contains("large") || path.contains("many") {
            return scenarios.get("large");
        }

        if path.contains("error") || path.contains("fail") {
            return scenarios.get("failure");
        }

        if path.contains("empty") || path.contains("none") {
            return scenarios.get("empty");
        }

        // Default to normal scenario
        scenarios.get("normal")
    }

    /// Generate scenario-specific completion templates
    pub fn generate_scenario_completions(scenario: &TestScenario) -> Vec<CompletionTemplate> {
        match scenario.name.as_str() {
            "Large Result Set" => Self::generate_large_completions(),
            "Slow Response" => Self::generate_slow_completions(),
            "Normal Response" => Self::generate_normal_completions(),
            "Empty Response" => Vec::new(),
            _ => Self::generate_normal_completions(),
        }
    }

    /// Generate a large number of completion templates for performance testing
    fn generate_large_completions() -> Vec<CompletionTemplate> {
        let mut completions = Vec::new();

        // Generate 100 different completion items
        for i in 0..100 {
            completions.push(CompletionTemplate {
                label: format!("large_completion_{:03}", i),
                kind: Self::cycle_completion_kind(i),
                detail: Some(format!("Large completion item #{}", i)),
                documentation: Some(format!(
                    "This is completion item #{} generated for large result set testing. \
                    It includes detailed documentation to test rendering performance.",
                    i
                )),
                insert_text: Some(format!("large_completion_{}()", i)),
                filter_text: Some(format!("large_completion_{}", i)),
                sort_text: Some(format!("{:03}", i)),
                triggers: vec![".".to_string(), " ".to_string()],
            });
        }

        completions
    }

    /// Generate completions for slow response testing
    fn generate_slow_completions() -> Vec<CompletionTemplate> {
        vec![
            CompletionTemplate {
                label: "slow_function_1".to_string(),
                kind: "Function".to_string(),
                detail: Some("fn slow_function_1() -> Result<()>".to_string()),
                documentation: Some(
                    "A function that simulates slow completion response".to_string(),
                ),
                insert_text: Some("slow_function_1()".to_string()),
                filter_text: Some("slow_function_1".to_string()),
                sort_text: Some("0001".to_string()),
                triggers: vec![".".to_string()],
            },
            CompletionTemplate {
                label: "slow_variable_1".to_string(),
                kind: "Variable".to_string(),
                detail: Some("slow_variable_1: String".to_string()),
                documentation: Some("A variable for slow completion testing".to_string()),
                insert_text: Some("slow_variable_1".to_string()),
                filter_text: Some("slow_variable_1".to_string()),
                sort_text: Some("0002".to_string()),
                triggers: vec![".".to_string(), " ".to_string()],
            },
            CompletionTemplate {
                label: "slow_method_1".to_string(),
                kind: "Method".to_string(),
                detail: Some("slow_method_1(&self) -> bool".to_string()),
                documentation: Some("A method for slow completion testing".to_string()),
                insert_text: Some("slow_method_1()".to_string()),
                filter_text: Some("slow_method_1".to_string()),
                sort_text: Some("0003".to_string()),
                triggers: vec![".".to_string()],
            },
        ]
    }

    /// Generate normal completions for standard testing
    fn generate_normal_completions() -> Vec<CompletionTemplate> {
        vec![
            CompletionTemplate {
                label: "test_function".to_string(),
                kind: "Function".to_string(),
                detail: Some("fn test_function() -> String".to_string()),
                documentation: Some("A standard test function for completion testing".to_string()),
                insert_text: Some("test_function()".to_string()),
                filter_text: Some("test_function".to_string()),
                sort_text: Some("0001".to_string()),
                triggers: vec![".".to_string(), " ".to_string()],
            },
            CompletionTemplate {
                label: "test_variable".to_string(),
                kind: "Variable".to_string(),
                detail: Some("test_variable: i32".to_string()),
                documentation: Some("A standard test variable".to_string()),
                insert_text: Some("test_variable".to_string()),
                filter_text: Some("test_variable".to_string()),
                sort_text: Some("0002".to_string()),
                triggers: vec![" ".to_string()],
            },
            CompletionTemplate {
                label: "test_method".to_string(),
                kind: "Method".to_string(),
                detail: Some("test_method(&self) -> Option<T>".to_string()),
                documentation: Some("A standard test method with generic return type".to_string()),
                insert_text: Some("test_method()".to_string()),
                filter_text: Some("test_method".to_string()),
                sort_text: Some("0003".to_string()),
                triggers: vec![".".to_string()],
            },
            CompletionTemplate {
                label: "test_keyword".to_string(),
                kind: "Keyword".to_string(),
                detail: Some("test_keyword".to_string()),
                documentation: Some("A standard test keyword".to_string()),
                insert_text: Some("test_keyword".to_string()),
                filter_text: Some("test_keyword".to_string()),
                sort_text: Some("0004".to_string()),
                triggers: vec![" ".to_string()],
            },
            CompletionTemplate {
                label: "test_snippet".to_string(),
                kind: "Snippet".to_string(),
                detail: Some("Test snippet with placeholders".to_string()),
                documentation: Some(
                    "A test snippet with multiple placeholders for testing snippet expansion"
                        .to_string(),
                ),
                insert_text: Some("test_snippet(${1:param1}, ${2:param2})$0".to_string()),
                filter_text: Some("test_snippet".to_string()),
                sort_text: Some("0005".to_string()),
                triggers: vec![" ".to_string()],
            },
        ]
    }

    /// Cycle through different completion kinds for variety
    fn cycle_completion_kind(index: usize) -> String {
        let kinds = [
            "Function",
            "Method",
            "Variable",
            "Field",
            "Property",
            "Class",
            "Interface",
            "Module",
            "Constant",
            "Enum",
            "Keyword",
            "Snippet",
            "Text",
            "Value",
            "Unit",
        ];

        kinds[index % kinds.len()].to_string()
    }
}

/// Scenario-based error generator for testing error handling
pub struct ErrorScenarioGenerator;

impl ErrorScenarioGenerator {
    /// Generate LSP errors for different test scenarios
    pub fn generate_error(scenario: &TestScenario) -> Option<lsp_server::ResponseError> {
        if !scenario.should_fail {
            return None;
        }

        let message = scenario
            .error_message
            .as_deref()
            .unwrap_or("Test LSP server error");

        Some(lsp_server::ResponseError {
            code: lsp_server::ErrorCode::InternalError as i32,
            message: message.to_string(),
            data: Some(serde_json::json!({
                "scenario": scenario.name,
                "description": scenario.description,
                "test_error": true
            })),
        })
    }

    /// Generate different types of LSP errors for comprehensive testing
    pub fn generate_specific_error(error_type: &str) -> lsp_server::ResponseError {
        match error_type {
            "timeout" => lsp_server::ResponseError {
                code: lsp_server::ErrorCode::RequestCanceled as i32,
                message: "Request timed out".to_string(),
                data: Some(serde_json::json!({"error_type": "timeout"})),
            },
            "invalid_params" => lsp_server::ResponseError {
                code: lsp_server::ErrorCode::InvalidParams as i32,
                message: "Invalid completion parameters".to_string(),
                data: Some(serde_json::json!({"error_type": "invalid_params"})),
            },
            "server_not_initialized" => lsp_server::ResponseError {
                code: lsp_server::ErrorCode::ServerNotInitialized as i32,
                message: "Server not properly initialized".to_string(),
                data: Some(serde_json::json!({"error_type": "not_initialized"})),
            },
            "method_not_found" => lsp_server::ResponseError {
                code: lsp_server::ErrorCode::MethodNotFound as i32,
                message: "Completion method not supported".to_string(),
                data: Some(serde_json::json!({"error_type": "method_not_found"})),
            },
            _ => lsp_server::ResponseError {
                code: lsp_server::ErrorCode::InternalError as i32,
                message: "Generic test error".to_string(),
                data: Some(serde_json::json!({"error_type": "generic"})),
            },
        }
    }
}
