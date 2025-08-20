// ABOUTME: Mock LSP server implementations for testing
// ABOUTME: Provides controllable mock servers for comprehensive LSP testing scenarios

#[cfg(test)]
pub mod mock_lsp_servers {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use helix_lsp::LanguageServerId;
    use nucleotide_events::ServerHealthStatus;
    use nucleotide_logging::{debug, error, info, warn};
    use serde_json::{Value as JsonValue, json};
    use tokio::sync::{Mutex, RwLock, mpsc};
    use tokio::time::sleep;

    use crate::ProjectLspError;

    /// Mock LSP server behavior configuration
    #[derive(Debug, Clone)]
    pub struct MockServerBehavior {
        /// Startup delay simulation
        pub startup_delay: Duration,
        /// Should the server fail to start?
        pub startup_failure: bool,
        /// Startup failure reason
        pub startup_error: Option<String>,
        /// Response delay for requests
        pub response_delay: Duration,
        /// Health check response
        pub health_status: ServerHealthStatus,
        /// Should health checks fail intermittently?
        pub intermittent_health_failure: bool,
        /// Memory usage simulation (MB)
        pub memory_usage_mb: u64,
        /// CPU usage simulation (percentage)
        pub cpu_usage_percent: f32,
    }

    impl Default for MockServerBehavior {
        fn default() -> Self {
            Self {
                startup_delay: Duration::from_millis(100),
                startup_failure: false,
                startup_error: None,
                response_delay: Duration::from_millis(10),
                health_status: ServerHealthStatus::Healthy,
                intermittent_health_failure: false,
                memory_usage_mb: 50,
                cpu_usage_percent: 2.5,
            }
        }
    }

    /// Mock LSP server instance
    #[derive(Debug, Clone)]
    pub struct MockLspServer {
        pub id: LanguageServerId,
        pub name: String,
        pub language_id: String,
        pub workspace_root: PathBuf,
        pub behavior: MockServerBehavior,
        pub capabilities: JsonValue,

        // State tracking
        pub started_at: Option<Instant>,
        pub last_request_at: Option<Instant>,
        pub request_count: Arc<RwLock<u64>>,
        pub is_initialized: Arc<RwLock<bool>>,
        pub health_check_count: Arc<RwLock<u64>>,

        // Communication channels
        pub request_tx: mpsc::UnboundedSender<MockLspRequest>,
        #[allow(dead_code)]
        pub request_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<MockLspRequest>>>>,
    }

    /// Mock LSP request types
    #[derive(Debug, Clone)]
    pub enum MockLspRequest {
        Initialize,
        TextDocumentDidOpen { uri: String, text: String },
        TextDocumentDidChange { uri: String, changes: Vec<String> },
        TextDocumentCompletion { uri: String, position: (u32, u32) },
        TextDocumentHover { uri: String, position: (u32, u32) },
        Shutdown,
        HealthCheck,
    }

    /// Mock LSP response types
    #[derive(Debug, Clone)]
    pub enum MockLspResponse {
        InitializeResult {
            capabilities: JsonValue,
        },
        CompletionResult {
            items: Vec<String>,
        },
        HoverResult {
            content: Option<String>,
        },
        Error {
            code: i32,
            message: String,
        },
        HealthStatus {
            status: ServerHealthStatus,
            metrics: JsonValue,
        },
    }

    impl MockLspServer {
        /// Create a new mock LSP server
        pub fn new(name: String, language_id: String, workspace_root: PathBuf) -> Self {
            let id = slotmap::KeyData::from_ffi(rand::random::<u64>()).into();
            let (request_tx, request_rx) = mpsc::unbounded_channel();

            let capabilities = json!({
                "textDocumentSync": 1,
                "completionProvider": {
                    "triggerCharacters": ["."]
                },
                "hoverProvider": true,
                "definitionProvider": true,
                "referencesProvider": true
            });

            Self {
                id,
                name,
                language_id,
                workspace_root,
                behavior: MockServerBehavior::default(),
                capabilities,
                started_at: None,
                last_request_at: None,
                request_count: Arc::new(RwLock::new(0)),
                is_initialized: Arc::new(RwLock::new(false)),
                health_check_count: Arc::new(RwLock::new(0)),
                request_tx,
                request_rx: Arc::new(Mutex::new(Some(request_rx))),
            }
        }

        /// Configure server behavior
        pub fn with_behavior(mut self, behavior: MockServerBehavior) -> Self {
            self.behavior = behavior;
            self
        }

        /// Start the mock server
        pub async fn start(&mut self) -> Result<(), ProjectLspError> {
            info!(
                server_id = ?self.id,
                server_name = %self.name,
                workspace_root = %self.workspace_root.display(),
                "Starting mock LSP server"
            );

            // Simulate startup delay
            if self.behavior.startup_delay > Duration::ZERO {
                sleep(self.behavior.startup_delay).await;
            }

            // Check for startup failure
            if self.behavior.startup_failure {
                let error_msg = self
                    .behavior
                    .startup_error
                    .clone()
                    .unwrap_or_else(|| "Mock server startup failure".to_string());
                error!(error = %error_msg, "Mock server startup failed");
                return Err(ProjectLspError::ServerStartup(error_msg));
            }

            self.started_at = Some(Instant::now());
            *self.is_initialized.write().await = true;

            info!(server_id = ?self.id, "Mock LSP server started successfully");
            Ok(())
        }

        /// Stop the mock server
        pub async fn stop(&mut self) -> Result<(), ProjectLspError> {
            info!(server_id = ?self.id, "Stopping mock LSP server");

            *self.is_initialized.write().await = false;

            info!(server_id = ?self.id, "Mock LSP server stopped");
            Ok(())
        }

        /// Send a request to the server
        pub async fn send_request(
            &self,
            request: MockLspRequest,
        ) -> Result<MockLspResponse, ProjectLspError> {
            if !*self.is_initialized.read().await {
                return Err(ProjectLspError::ServerCommunication(
                    "Server not initialized".to_string(),
                ));
            }

            // Simulate response delay
            if self.behavior.response_delay > Duration::ZERO {
                sleep(self.behavior.response_delay).await;
            }

            // Increment request count
            *self.request_count.write().await += 1;

            debug!(request = ?request, "Processing mock LSP request");

            let response = match request {
                MockLspRequest::Initialize => MockLspResponse::InitializeResult {
                    capabilities: self.capabilities.clone(),
                },

                MockLspRequest::TextDocumentCompletion { uri, position } => {
                    let items = vec![
                        format!("completion_item_1_at_{}:{}", position.0, position.1),
                        format!("completion_item_2_for_{}", uri),
                        "mock_completion_item".to_string(),
                    ];
                    MockLspResponse::CompletionResult { items }
                }

                MockLspRequest::TextDocumentHover { uri, position: _ } => {
                    let content = if uri.ends_with(".rs") {
                        Some("Mock hover content for Rust".to_string())
                    } else if uri.ends_with(".ts") {
                        Some("Mock hover content for TypeScript".to_string())
                    } else {
                        None
                    };
                    MockLspResponse::HoverResult { content }
                }

                MockLspRequest::HealthCheck => {
                    *self.health_check_count.write().await += 1;

                    let status = if self.behavior.intermittent_health_failure {
                        let health_checks = *self.health_check_count.read().await;
                        if health_checks % 3 == 0 {
                            ServerHealthStatus::Failed {
                                error: "Intermittent health check failure".to_string(),
                            }
                        } else {
                            self.behavior.health_status.clone()
                        }
                    } else {
                        self.behavior.health_status.clone()
                    };

                    let metrics = json!({
                        "memory_usage_mb": self.behavior.memory_usage_mb,
                        "cpu_usage_percent": self.behavior.cpu_usage_percent,
                        "request_count": *self.request_count.read().await,
                        "uptime_seconds": self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0)
                    });

                    MockLspResponse::HealthStatus { status, metrics }
                }

                _ => MockLspResponse::Error {
                    code: -32601,
                    message: "Method not implemented in mock server".to_string(),
                },
            };

            Ok(response)
        }

        /// Get server statistics
        pub async fn get_stats(&self) -> JsonValue {
            json!({
                "server_id": format!("{:?}", self.id),
                "name": self.name,
                "language_id": self.language_id,
                "workspace_root": self.workspace_root.display().to_string(),
                "is_initialized": *self.is_initialized.read().await,
                "uptime_seconds": self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0),
                "request_count": *self.request_count.read().await,
                "health_check_count": *self.health_check_count.read().await,
                "memory_usage_mb": self.behavior.memory_usage_mb,
                "cpu_usage_percent": self.behavior.cpu_usage_percent
            })
        }

        /// Check if server is healthy
        pub async fn is_healthy(&self) -> bool {
            if self.behavior.intermittent_health_failure {
                let health_checks = *self.health_check_count.read().await;
                health_checks % 3 != 0
            } else {
                matches!(self.behavior.health_status, ServerHealthStatus::Healthy)
            }
        }

        /// Simulate server crash
        pub async fn simulate_crash(&mut self) {
            warn!(server_id = ?self.id, "Simulating server crash");
            *self.is_initialized.write().await = false;
            self.behavior.health_status = ServerHealthStatus::Crashed;
        }

        /// Simulate high resource usage
        pub fn simulate_high_resource_usage(&mut self) {
            self.behavior.memory_usage_mb = 500; // High memory usage
            self.behavior.cpu_usage_percent = 80.0; // High CPU usage
            self.behavior.response_delay = Duration::from_millis(1000); // Slow responses
        }
    }

    /// Mock server registry for managing multiple servers
    #[derive(Debug)]
    pub struct MockServerRegistry {
        servers: Arc<RwLock<HashMap<LanguageServerId, MockLspServer>>>,
        server_count_by_type: Arc<RwLock<HashMap<String, usize>>>,
    }

    impl MockServerRegistry {
        pub fn new() -> Self {
            Self {
                servers: Arc::new(RwLock::new(HashMap::new())),
                server_count_by_type: Arc::new(RwLock::new(HashMap::new())),
            }
        }

        /// Register a server in the registry
        pub async fn register_server(&self, server: MockLspServer) {
            let server_id = server.id;
            let server_type = server.name.clone();

            // Update type count
            {
                let mut counts = self.server_count_by_type.write().await;
                *counts.entry(server_type).or_insert(0) += 1;
            }

            self.servers.write().await.insert(server_id, server);
            info!(server_id = ?server_id, "Registered mock server");
        }

        /// Unregister a server from the registry
        pub async fn unregister_server(
            &self,
            server_id: LanguageServerId,
        ) -> Result<(), ProjectLspError> {
            let server = {
                let mut servers = self.servers.write().await;
                servers.remove(&server_id)
            };

            if let Some(server) = server {
                // Update type count
                {
                    let mut counts = self.server_count_by_type.write().await;
                    if let Some(count) = counts.get_mut(&server.name) {
                        *count = count.saturating_sub(1);
                        if *count == 0 {
                            counts.remove(&server.name);
                        }
                    }
                }

                info!(server_id = ?server_id, "Unregistered mock server");
                Ok(())
            } else {
                Err(ProjectLspError::Internal(format!(
                    "Server {:?} not found in registry",
                    server_id
                )))
            }
        }

        /// Get server by ID
        pub async fn get_server(&self, server_id: LanguageServerId) -> Option<MockLspServer> {
            self.servers.read().await.get(&server_id).cloned()
        }

        /// Get all servers
        pub async fn get_all_servers(&self) -> Vec<MockLspServer> {
            self.servers.read().await.values().cloned().collect()
        }

        /// Get servers by type
        pub async fn get_servers_by_type(&self, server_type: &str) -> Vec<MockLspServer> {
            self.servers
                .read()
                .await
                .values()
                .filter(|server| server.name == server_type)
                .cloned()
                .collect()
        }

        /// Get server count by type
        pub async fn get_server_count_by_type(&self, server_type: &str) -> usize {
            self.server_count_by_type
                .read()
                .await
                .get(server_type)
                .copied()
                .unwrap_or(0)
        }

        /// Get total server count
        pub async fn get_total_server_count(&self) -> usize {
            self.servers.read().await.len()
        }

        /// Shutdown all servers
        pub async fn shutdown_all(&mut self) -> Vec<Result<(), ProjectLspError>> {
            let mut results = Vec::new();
            let server_ids: Vec<LanguageServerId> =
                { self.servers.read().await.keys().copied().collect() };

            for server_id in server_ids {
                if let Some(mut server) = self.servers.write().await.remove(&server_id) {
                    let result = server.stop().await;
                    results.push(result);
                }
            }

            self.server_count_by_type.write().await.clear();
            info!("All mock servers shutdown");

            results
        }

        /// Perform health check on all servers
        pub async fn health_check_all(&self) -> HashMap<LanguageServerId, ServerHealthStatus> {
            let mut results = HashMap::new();
            let servers = self.servers.read().await;

            for (&server_id, server) in servers.iter() {
                let is_healthy = server.is_healthy().await;
                let status = if is_healthy {
                    ServerHealthStatus::Healthy
                } else {
                    server.behavior.health_status.clone()
                };
                results.insert(server_id, status);
            }

            results
        }

        /// Get registry statistics
        pub async fn get_registry_stats(&self) -> JsonValue {
            let servers = self.servers.read().await;
            let type_counts = self.server_count_by_type.read().await;

            let total_requests: u64 = {
                let mut total = 0;
                for server in servers.values() {
                    total += *server.request_count.read().await;
                }
                total
            };

            json!({
                "total_servers": servers.len(),
                "servers_by_type": *type_counts,
                "total_requests": total_requests,
                "registry_uptime_seconds": 0 // Could add registry startup time tracking
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock_lsp_servers::*;
    use nucleotide_events::ServerHealthStatus;
    use std::path::PathBuf;
    use std::time::Duration;

    #[tokio::test]
    async fn test_mock_server_creation_and_startup() {
        let workspace_root = PathBuf::from("/test/workspace");
        let mut server = MockLspServer::new(
            "rust-analyzer".to_string(),
            "rust".to_string(),
            workspace_root.clone(),
        );

        // Test startup
        let result = server.start().await;
        assert!(result.is_ok(), "Mock server should start successfully");

        // Test that server is initialized
        assert!(
            *server.is_initialized.read().await,
            "Server should be initialized"
        );
        assert!(
            server.started_at.is_some(),
            "Server should have startup time"
        );
    }

    #[tokio::test]
    async fn test_mock_server_failure_scenarios() {
        let workspace_root = PathBuf::from("/test/workspace");
        let mut server = MockLspServer::new(
            "failing-server".to_string(),
            "unknown".to_string(),
            workspace_root,
        )
        .with_behavior(MockServerBehavior {
            startup_failure: true,
            startup_error: Some("Test failure".to_string()),
            ..Default::default()
        });

        // Test startup failure
        let result = server.start().await;
        assert!(result.is_err(), "Mock server should fail to start");

        match result {
            Err(crate::ProjectLspError::ServerStartup(msg)) => {
                assert_eq!(msg, "Test failure");
            }
            _ => panic!("Expected ServerStartup error"),
        }
    }

    #[tokio::test]
    async fn test_mock_server_request_handling() {
        let workspace_root = PathBuf::from("/test/workspace");
        let mut server = MockLspServer::new(
            "test-server".to_string(),
            "rust".to_string(),
            workspace_root,
        );

        server.start().await.expect("Server should start");

        // Test completion request
        let completion_request = MockLspRequest::TextDocumentCompletion {
            uri: "file:///test.rs".to_string(),
            position: (10, 5),
        };

        let response = server
            .send_request(completion_request)
            .await
            .expect("Request should succeed");

        match response {
            MockLspResponse::CompletionResult { items } => {
                assert!(!items.is_empty(), "Should have completion items");
                assert!(
                    items[0].contains("10:5"),
                    "Should include position in completion"
                );
            }
            _ => panic!("Expected completion result"),
        }

        // Verify request count
        assert_eq!(
            *server.request_count.read().await,
            1,
            "Should track request count"
        );
    }

    #[tokio::test]
    async fn test_mock_server_health_checks() {
        let workspace_root = PathBuf::from("/test/workspace");
        let mut server = MockLspServer::new(
            "health-test-server".to_string(),
            "rust".to_string(),
            workspace_root,
        )
        .with_behavior(MockServerBehavior {
            intermittent_health_failure: true,
            ..Default::default()
        });

        server.start().await.expect("Server should start");

        // Test multiple health checks to see intermittent failures
        let mut healthy_count = 0;
        let mut failed_count = 0;

        for _ in 0..6 {
            let health_request = MockLspRequest::HealthCheck;
            let response = server
                .send_request(health_request)
                .await
                .expect("Health check should succeed");

            match response {
                MockLspResponse::HealthStatus { status, .. } => match status {
                    ServerHealthStatus::Healthy => healthy_count += 1,
                    ServerHealthStatus::Failed { .. } => failed_count += 1,
                    _ => {}
                },
                _ => panic!("Expected health status response"),
            }
        }

        assert!(healthy_count > 0, "Should have some healthy responses");
        assert!(
            failed_count > 0,
            "Should have some failed responses due to intermittent failure"
        );
    }

    #[tokio::test]
    async fn test_mock_server_registry() {
        let mut registry = MockServerRegistry::new();

        // Create and register servers
        let workspace_root = PathBuf::from("/test/workspace");
        let server1 = MockLspServer::new(
            "rust-analyzer".to_string(),
            "rust".to_string(),
            workspace_root.clone(),
        );
        let server1_id = server1.id;

        let server2 = MockLspServer::new(
            "typescript-language-server".to_string(),
            "typescript".to_string(),
            workspace_root,
        );
        let _server2_id = server2.id;

        registry.register_server(server1).await;
        registry.register_server(server2).await;

        // Test registry functionality
        assert_eq!(registry.get_total_server_count().await, 2);
        assert_eq!(registry.get_server_count_by_type("rust-analyzer").await, 1);
        assert_eq!(
            registry
                .get_server_count_by_type("typescript-language-server")
                .await,
            1
        );

        // Test server retrieval
        let retrieved_server = registry.get_server(server1_id).await;
        assert!(
            retrieved_server.is_some(),
            "Should retrieve registered server"
        );

        // Test unregistration
        registry
            .unregister_server(server1_id)
            .await
            .expect("Should unregister server");

        assert_eq!(registry.get_total_server_count().await, 1);
        assert_eq!(registry.get_server_count_by_type("rust-analyzer").await, 0);

        // Test shutdown all
        let results = registry.shutdown_all().await;
        assert_eq!(results.len(), 1, "Should shutdown remaining server");
        assert_eq!(registry.get_total_server_count().await, 0);
    }

    #[tokio::test]
    async fn test_mock_server_performance_characteristics() {
        let workspace_root = PathBuf::from("/test/workspace");
        let mut server = MockLspServer::new(
            "performance-test-server".to_string(),
            "rust".to_string(),
            workspace_root,
        )
        .with_behavior(MockServerBehavior {
            startup_delay: Duration::from_millis(50),
            response_delay: Duration::from_millis(10),
            ..Default::default()
        });

        // Test startup performance
        let start = std::time::Instant::now();
        server.start().await.expect("Server should start");
        let startup_time = start.elapsed();

        assert!(
            startup_time >= Duration::from_millis(45),
            "Should respect startup delay"
        );
        assert!(
            startup_time < Duration::from_millis(100),
            "Startup should not be too slow"
        );

        // Test request response time
        let request_start = std::time::Instant::now();
        let _response = server
            .send_request(MockLspRequest::Initialize)
            .await
            .expect("Request should succeed");
        let request_time = request_start.elapsed();

        assert!(
            request_time >= Duration::from_millis(5),
            "Should respect response delay"
        );
        assert!(
            request_time < Duration::from_millis(50),
            "Response should not be too slow"
        );
    }

    #[tokio::test]
    async fn test_mock_server_resource_simulation() {
        let workspace_root = PathBuf::from("/test/workspace");
        let mut server = MockLspServer::new(
            "resource-test-server".to_string(),
            "rust".to_string(),
            workspace_root,
        );

        server.start().await.expect("Server should start");

        // Test normal resource usage
        let stats = server.get_stats().await;
        assert_eq!(stats["memory_usage_mb"], 50);
        assert_eq!(stats["cpu_usage_percent"], 2.5);

        // Test high resource usage simulation
        server.simulate_high_resource_usage();
        let high_stats = server.get_stats().await;
        assert_eq!(high_stats["memory_usage_mb"], 500);
        assert_eq!(high_stats["cpu_usage_percent"], 80.0);
    }
}
