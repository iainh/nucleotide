// ABOUTME: Project markers configuration types for custom project detection
// ABOUTME: Shared configuration types between nucleotide and nucleotide-lsp crates

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root detection strategy for project markers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RootStrategy {
    /// Stop at first matching marker
    First,
    /// Use marker closest to file
    Closest,
    /// Use marker furthest from file
    Furthest,
}

impl Default for RootStrategy {
    fn default() -> Self {
        RootStrategy::Closest
    }
}

/// Individual project marker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMarker {
    /// File patterns that identify this project type
    pub markers: Vec<String>,

    /// Language server to start for this project type
    pub language_server: String,

    /// Root detection strategy
    #[serde(default)]
    pub root_strategy: RootStrategy,

    /// Priority for this marker (higher numbers = higher priority)
    #[serde(default = "default_priority")]
    pub priority: u32,
}

fn default_priority() -> u32 {
    50
}

impl ProjectMarker {
    /// Validate the project marker configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate markers are not empty
        if self.markers.is_empty() {
            return Err("Project marker must have at least one marker pattern".to_string());
        }

        // Validate marker patterns are not empty strings
        for (index, marker) in self.markers.iter().enumerate() {
            if marker.trim().is_empty() {
                return Err(format!("Marker pattern at index {} cannot be empty", index));
            }

            // Validate marker patterns don't contain path separators
            if marker.contains('/') || marker.contains('\\') {
                return Err(format!(
                    "Marker pattern '{}' cannot contain path separators - use glob patterns instead",
                    marker
                ));
            }
        }

        // Validate language server name is not empty
        if self.language_server.trim().is_empty() {
            return Err("Language server name cannot be empty".to_string());
        }

        Ok(())
    }

    /// Get a sanitized version of the project marker with valid values
    pub fn sanitized(&self) -> Self {
        let mut marker = self.clone();

        // Filter out empty markers
        marker.markers = marker
            .markers
            .into_iter()
            .filter(|m| !m.trim().is_empty())
            .collect();

        // Ensure at least one marker exists
        if marker.markers.is_empty() {
            marker.markers = vec![".project".to_string()];
        }

        // Sanitize language server name
        marker.language_server = marker.language_server.trim().to_string();
        if marker.language_server.is_empty() {
            marker.language_server = "unknown".to_string();
        }

        // Ensure priority is reasonable
        if marker.priority > 1000 {
            marker.priority = 1000;
        }

        marker
    }
}

/// Project markers configuration for custom project detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMarkersConfig {
    /// Map of project type names to their marker configurations
    #[serde(default)]
    pub markers: HashMap<String, ProjectMarker>,

    /// Enable project markers for LSP startup
    #[serde(default)]
    pub enable_project_markers: bool,

    /// Timeout for project detection in milliseconds
    #[serde(default = "default_project_detection_timeout")]
    pub detection_timeout_ms: u64,

    /// Enable fallback to built-in project detection
    #[serde(default = "default_true")]
    pub enable_builtin_fallback: bool,
}

impl Default for ProjectMarkersConfig {
    fn default() -> Self {
        Self {
            markers: HashMap::new(),
            enable_project_markers: false,
            detection_timeout_ms: default_project_detection_timeout(),
            enable_builtin_fallback: default_true(),
        }
    }
}

fn default_project_detection_timeout() -> u64 {
    1000 // 1 second default timeout
}

fn default_true() -> bool {
    true
}

impl ProjectMarkersConfig {
    /// Validate the project markers configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate detection timeout is reasonable
        if self.detection_timeout_ms == 0 {
            return Err("Project detection timeout must be greater than 0".to_string());
        }

        if self.detection_timeout_ms > 30000 {
            return Err("Project detection timeout should not exceed 30 seconds".to_string());
        }

        // Validate each project marker
        for (project_name, marker) in &self.markers {
            if project_name.trim().is_empty() {
                return Err("Project type name cannot be empty".to_string());
            }

            if let Err(validation_error) = marker.validate() {
                return Err(format!(
                    "Invalid configuration for project type '{}': {}",
                    project_name, validation_error
                ));
            }
        }

        Ok(())
    }

    /// Get a sanitized version of the configuration with valid values
    pub fn sanitized(&self) -> Self {
        let mut config = self.clone();

        // Ensure timeout is within reasonable bounds
        if config.detection_timeout_ms == 0 {
            config.detection_timeout_ms = 1000;
        } else if config.detection_timeout_ms > 30000 {
            config.detection_timeout_ms = 30000;
        }

        // Sanitize each project marker
        let mut sanitized_markers = HashMap::new();
        for (project_name, marker) in config.markers {
            let project_name = project_name.trim().to_string();
            if !project_name.is_empty() {
                sanitized_markers.insert(project_name, marker.sanitized());
            }
        }
        config.markers = sanitized_markers;

        config
    }

    /// Get all configured language servers
    pub fn get_language_servers(&self) -> Vec<String> {
        let mut servers: Vec<String> = self
            .markers
            .values()
            .map(|marker| marker.language_server.clone())
            .collect();
        servers.sort();
        servers.dedup();
        servers
    }

    /// Get markers for a specific language server
    pub fn get_markers_for_server(&self, server_name: &str) -> Vec<(String, &ProjectMarker)> {
        let mut results = Vec::new();
        for (project_name, marker) in &self.markers {
            if marker.language_server == server_name {
                results.push((project_name.clone(), marker));
            }
        }
        // Sort by priority (descending)
        results.sort_by(|a, b| b.1.priority.cmp(&a.1.priority));
        results
    }

    /// Find project type by marker pattern
    pub fn find_project_by_marker(&self, marker_pattern: &str) -> Vec<(String, &ProjectMarker)> {
        let mut matches = Vec::new();
        for (project_name, marker) in &self.markers {
            if marker.markers.contains(&marker_pattern.to_string()) {
                matches.push((project_name.clone(), marker));
            }
        }
        // Sort by priority (descending)
        matches.sort_by(|a, b| b.1.priority.cmp(&a.1.priority));
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_root_strategy_serialization() {
        assert_eq!(RootStrategy::default(), RootStrategy::Closest);

        // Test serialization round-trip
        let first = RootStrategy::First;
        let serialized = serde_json::to_string(&first).unwrap();
        let deserialized: RootStrategy = serde_json::from_str(&serialized).unwrap();
        assert_eq!(first, deserialized);
    }

    #[test]
    fn test_project_marker_parsing() {
        let config_str = r#"
{
    "markers": ["Cargo.toml", "leptos.toml"],
    "language_server": "rust-analyzer",
    "root_strategy": "closest",
    "priority": 80
}
"#;

        let marker: ProjectMarker =
            serde_json::from_str(config_str).expect("Failed to parse ProjectMarker");

        assert_eq!(marker.markers, vec!["Cargo.toml", "leptos.toml"]);
        assert_eq!(marker.language_server, "rust-analyzer");
        assert_eq!(marker.root_strategy, RootStrategy::Closest);
        assert_eq!(marker.priority, 80);
    }

    #[test]
    fn test_project_marker_validation() {
        // Valid marker
        let valid_marker = ProjectMarker {
            markers: vec!["Cargo.toml".to_string()],
            language_server: "rust-analyzer".to_string(),
            root_strategy: RootStrategy::Closest,
            priority: 50,
        };
        assert!(valid_marker.validate().is_ok());

        // Invalid - empty markers
        let invalid_marker = ProjectMarker {
            markers: vec![],
            language_server: "rust-analyzer".to_string(),
            root_strategy: RootStrategy::Closest,
            priority: 50,
        };
        assert!(invalid_marker.validate().is_err());

        // Invalid - empty language server
        let invalid_marker = ProjectMarker {
            markers: vec!["Cargo.toml".to_string()],
            language_server: "".to_string(),
            root_strategy: RootStrategy::Closest,
            priority: 50,
        };
        assert!(invalid_marker.validate().is_err());
    }

    #[test]
    fn test_project_markers_config_defaults() {
        let config = ProjectMarkersConfig::default();

        assert_eq!(config.enable_project_markers, false);
        assert_eq!(config.detection_timeout_ms, 1000);
        assert_eq!(config.enable_builtin_fallback, true);
        assert!(config.markers.is_empty());
    }

    #[test]
    fn test_project_markers_config_validation() {
        // Valid config
        let mut valid_config = ProjectMarkersConfig::default();
        valid_config.enable_project_markers = true;
        valid_config.detection_timeout_ms = 1000;
        valid_config.markers.insert(
            "rust".to_string(),
            ProjectMarker {
                markers: vec!["Cargo.toml".to_string()],
                language_server: "rust-analyzer".to_string(),
                root_strategy: RootStrategy::Closest,
                priority: 50,
            },
        );
        assert!(valid_config.validate().is_ok());

        // Invalid - zero timeout
        let mut invalid_config = ProjectMarkersConfig::default();
        invalid_config.detection_timeout_ms = 0;
        assert!(invalid_config.validate().is_err());
    }

    #[test]
    fn test_project_markers_config_utility_methods() {
        let mut config = ProjectMarkersConfig::default();
        config.markers.insert(
            "rust".to_string(),
            ProjectMarker {
                markers: vec!["Cargo.toml".to_string()],
                language_server: "rust-analyzer".to_string(),
                root_strategy: RootStrategy::Closest,
                priority: 80,
            },
        );
        config.markers.insert(
            "typescript".to_string(),
            ProjectMarker {
                markers: vec!["package.json".to_string(), "tsconfig.json".to_string()],
                language_server: "typescript-language-server".to_string(),
                root_strategy: RootStrategy::First,
                priority: 70,
            },
        );

        // Test get_language_servers
        let servers = config.get_language_servers();
        assert_eq!(servers.len(), 2);
        assert!(servers.contains(&"rust-analyzer".to_string()));
        assert!(servers.contains(&"typescript-language-server".to_string()));

        // Test get_markers_for_server
        let rust_markers = config.get_markers_for_server("rust-analyzer");
        assert_eq!(rust_markers.len(), 1);
        assert_eq!(rust_markers[0].0, "rust");

        // Test find_project_by_marker
        let cargo_projects = config.find_project_by_marker("Cargo.toml");
        assert_eq!(cargo_projects.len(), 1);
        assert_eq!(cargo_projects[0].0, "rust");
    }
}
