# Nucleotide Logging Implementation Prompts

This document contains step-by-step implementation prompts for building the comprehensive logging solution for Nucleotide.

## Phase 1: Foundation

### Step 1: Create nucleotide-logging Crate

```
Create a new crate called `nucleotide-logging` in the workspace with the following requirements:

1. Add to the workspace in `Cargo.toml` 
2. Create `crates/nucleotide-logging/` with basic crate structure
3. Add these dependencies to the crate:
   - `tracing` (core tracing functionality)
   - `tracing-subscriber` (subscriber implementations and utilities) 
   - `tracing-appender` (file appender with rotation)
   - `serde` with derive feature (for configuration)
   - `dirs` (for finding config directories)
   - `anyhow` (error handling)

4. Create the crate's main module structure:
   - `lib.rs` - public API
   - `subscriber.rs` - subscriber configuration and setup
   - `config.rs` - logging configuration structures
   - `layers.rs` - custom layer implementations
   
5. Export a simple `init_logging()` function that sets up basic tracing with console output
6. Add the new crate to workspace dependencies
7. Ensure the crate compiles successfully
```

### Step 2: Implement Configurable Subscriber

```
Implement a flexible tracing subscriber configuration in the `nucleotide-logging` crate:

1. In `config.rs`, create a `LoggingConfig` struct that supports:
   - Log level configuration (trace, debug, info, warn, error)
   - Output targets (console, file, json)  
   - File path configuration (defaulting to `~/.config/nucleotide/nucleotide.log`)
   - Environment variable override support (`NUCLEOTIDE_LOG`, `RUST_LOG`)
   - Per-module log level configuration

2. In `layers.rs`, implement layer creation functions:
   - Console layer with pretty formatting for development
   - File layer with rotation and structured output
   - JSON layer for machine-readable output
   - Filtering layer for performance

3. In `subscriber.rs`, implement:
   - `init_subscriber(config: LoggingConfig)` function
   - Logic to combine multiple layers based on config
   - Proper error handling for file creation and permissions
   - Registry setup with layer composition

4. Update `lib.rs` to provide a clean public API:
   - Re-export core tracing macros (`info!`, `debug!`, etc.)
   - Export configuration structs
   - Export initialization functions

5. Add unit tests for configuration parsing and validation
6. Ensure all code compiles and basic tests pass
```

### Step 3: Update Workspace Dependencies

```
Update the workspace to use the new logging infrastructure:

1. Add `nucleotide-logging` as a workspace dependency in root `Cargo.toml`
2. Update each crate's `Cargo.toml` to include:
   - Remove direct `log` dependency where present
   - Add `nucleotide-logging` dependency
   - Keep `tracing` as workspace dependency for macro usage

3. Add these new dependencies to workspace dependencies:
   - `tracing = "0.1"`
   - `tracing-subscriber = "0.3"`
   - `tracing-appender = "0.2"`
   - `dirs = "5.0"`

4. Verify that all crates still compile with the new dependencies
5. Run `cargo check` on the entire workspace to ensure no dependency conflicts
6. Update any existing logging-related imports to use the new crate
```

### Step 4: Basic Integration Test

```
Create basic integration tests and smoke tests for the logging system:

1. In the `nucleotide-logging` crate, add integration tests in `tests/`:
   - Test basic subscriber initialization
   - Test file logging creation and writing
   - Test configuration parsing from environment variables
   - Test multi-layer subscriber setup

2. Create a simple CLI test harness:
   - Small binary that initializes logging with different configs
   - Emits test log messages at different levels
   - Verifies output appears in expected locations

3. Update the main `nucleotide` binary to use the new logging:
   - Replace the `setup_logging()` function in `main.rs`
   - Initialize the new subscriber before starting the application
   - Preserve existing verbosity level behavior from command line args

4. Test the integration:
   - Run the application and verify console output works
   - Check that log file is created at `~/.config/nucleotide/nucleotide.log` 
   - Verify that different log levels are filtered correctly
   - Ensure no regression in existing functionality

5. Fix any issues found during testing before proceeding to next phase
```

## Phase 2: Core Migration

### Step 5: Migrate nucleotide-core Event Bridges

```
Migrate the event bridge system in `nucleotide-core` to use structured tracing:

1. Update imports in these files to use tracing instead of log:
   - `src/event_bridge.rs`
   - `src/gpui_to_helix_bridge.rs`

2. Add structured spans to key operations:
   - Wrap `send_event` operations with spans including event type and context
   - Add spans around bridge initialization and teardown
   - Include structured fields: `event.type`, `bridge.direction`, `component`

3. Replace log macros with tracing equivalents:
   - `log::info!` → `tracing::info!`
   - `log::debug!` → `tracing::debug!`
   - `log::warn!` → `tracing::warn!`
   - `log::error!` → `tracing::error!`

4. Add contextual information to log statements:
   - Include relevant IDs (document_id, view_id, window_id)
   - Add timing information for performance-sensitive operations
   - Include error context and recovery actions

5. Instrument async operations:
   - Use `#[tracing::instrument]` on async functions where appropriate
   - Add manual spans around complex async workflows
   - Ensure proper span context propagation

6. Test the changes:
   - Verify event bridge operations produce structured logs
   - Check that spans properly nest and provide context
   - Ensure no performance regression in event handling
   - Validate that error scenarios are properly logged
```

### Step 6: Add Editor Operation Instrumentation

```
Add comprehensive tracing to document and editor operations:

1. Instrument key operations in `nucleotide-editor` crate:
   - Document loading and saving
   - Text modifications and cursor movements
   - Selection changes and text rendering
   - Scroll operations and viewport changes

2. Update these files with structured tracing:
   - `src/document_renderer.rs` - rendering pipeline tracing
   - `src/editor_view.rs` - view state change tracking
   - `src/scroll_manager.rs` - scroll operation tracing

3. Add these structured fields to spans:
   - `document.id` - unique document identifier
   - `document.path` - file path being edited
   - `operation.type` - type of editor operation
   - `view.id` - editor view identifier
   - `cursor.line` and `cursor.col` - cursor position
   - `selection.ranges` - current selection information

4. Instrument performance-critical operations:
   - Text rendering with timing spans
   - Syntax highlighting operations
   - Large file operations with progress tracking

5. Add error context to editor operations:
   - File access errors with path and permissions info
   - Syntax highlighting errors with grammar context
   - Rendering failures with recovery actions

6. Test editor instrumentation:
   - Verify operations produce meaningful trace data
   - Check performance impact is minimal
   - Ensure error scenarios provide useful debugging info
   - Test with various document sizes and types
```

### Step 7: Instrument LSP Communication

```
Add structured tracing to LSP interactions in `nucleotide-lsp`:

1. Instrument LSP lifecycle in these files:
   - `src/lsp_manager.rs` - server management and lifecycle
   - `src/lsp_state.rs` - state tracking and synchronization
   - `src/lsp_status.rs` - status reporting and progress

2. Add spans for LSP request/response cycles:
   - Create span for each LSP request with method and params
   - Track request duration and response size
   - Include server identification and document context

3. Add these structured fields:
   - `lsp.server.id` - LSP server identifier
   - `lsp.method` - LSP method name (textDocument/hover, etc.)
   - `lsp.request_id` - unique request identifier
   - `document.uri` - document being operated on
   - `lsp.response.duration_ms` - request timing
   - `lsp.server.status` - server status (starting, running, error)

4. Instrument server lifecycle events:
   - Server startup and initialization with configuration details
   - Server shutdown and restart operations
   - Progress notifications with detailed status

5. Add comprehensive error logging:
   - Server crashes with error codes and recovery actions
   - Request timeouts with context and retry information
   - Communication errors with diagnostic information

6. Test LSP instrumentation:
   - Verify request/response tracing works correctly
   - Check server lifecycle events are properly logged
   - Test error scenarios and recovery logging
   - Ensure minimal performance impact on LSP operations
```

### Step 8: Update Error Handling with Context

```
Enhance error handling across all migrated components with structured context:

1. Update error handling patterns to include tracing context:
   - Use `tracing::error!` with structured fields for all errors
   - Add `error.source` and `error.chain` information
   - Include recovery actions and user guidance

2. Create error handling utilities in `nucleotide-logging`:
   - Helper functions for common error scenarios
   - Structured error reporting with consistent fields
   - Integration with existing anyhow error chains

3. Add context preservation in async operations:
   - Ensure error context flows through async boundaries
   - Maintain span context in error propagation
   - Include timing and state information in error reports

4. Update error recovery scenarios:
   - Log recovery attempts and success/failure
   - Track error patterns and frequencies
   - Include user-visible error messages in traces

5. Add diagnostic information for common issues:
   - File permission errors with suggested fixes
   - Network timeouts with retry logic context
   - Configuration errors with validation details

6. Test error handling improvements:
   - Verify error scenarios produce useful structured logs
   - Check that error context is preserved across async boundaries
   - Test error recovery logging
   - Ensure error logs provide actionable information for debugging
```

## Phase 3: Application Integration

### Step 9: Main Application Startup Integration

```
Integrate the new logging system into the main application startup process:

1. Update `main.rs` to use the new logging infrastructure:
   - Replace existing `setup_logging()` function completely
   - Initialize `nucleotide-logging` early in the startup process
   - Configure logging based on command-line arguments and environment

2. Add startup tracing spans:
   - Application initialization span with version and build info
   - Configuration loading span with config file paths
   - Runtime initialization span with feature flags

3. Implement configuration integration:
   - Read logging configuration from `~/.config/nucleotide/nucleotide.toml`
   - Support command-line overrides for log level and output
   - Environment variable support (`NUCLEOTIDE_LOG`, `RUST_LOG`)

4. Add structured fields for application context:
   - `app.version` - application version
   - `app.build.git_hash` - git commit hash
   - `app.config.path` - configuration file path
   - `app.runtime.features` - enabled feature flags
   - `app.platform` - operating system information

5. Instrument panic handling and graceful shutdown:
   - Log panic information with stack traces
   - Add shutdown tracing with cleanup operations
   - Include performance metrics in shutdown logs

6. Test application integration:
   - Verify logging starts correctly during application launch
   - Check configuration loading and override behavior
   - Test panic handling and shutdown logging
   - Ensure no regression in startup performance or behavior
```

### Step 10: UI and Workspace Operation Tracing

```
Add tracing to UI operations and workspace management:

1. Instrument `nucleotide-ui` components:
   - UI rendering and interaction tracking
   - Component lifecycle and state changes
   - User input processing and command dispatching

2. Update key UI files with tracing:
   - `src/picker_view.rs` - file picker operations
   - `src/completion.rs` - completion popup lifecycle
   - `src/theme_manager.rs` - theme loading and switching

3. Instrument `nucleotide-workspace` operations:
   - Workspace loading and project detection
   - Tab management and document switching
   - Layout changes and window management

4. Add these structured fields for UI operations:
   - `ui.component` - UI component name
   - `ui.action` - user action or system event
   - `ui.state` - current component state
   - `workspace.path` - current workspace path
   - `tab.count` - number of open tabs
   - `window.dimensions` - window size information

5. Instrument user interactions:
   - Key presses and command execution
   - Mouse clicks and selections
   - Menu interactions and preferences changes

6. Add performance monitoring:
   - Frame rendering time tracking
   - UI responsiveness measurements
   - Memory usage during UI operations

7. Test UI and workspace tracing:
   - Verify user interactions generate appropriate traces
   - Check performance impact is minimal
   - Test workspace operations produce useful debug information
   - Ensure UI state changes are properly logged
```

### Step 11: File Operations Instrumentation

```
Add comprehensive tracing to file system operations and watching:

1. Instrument file operations in the file tree:
   - `src/file_tree/view.rs` - tree rendering and interaction
   - `src/file_tree/watcher.rs` - file system watching
   - `src/file_tree/tree.rs` - tree data structure operations

2. Add spans for file system operations:
   - File and directory loading with timing
   - File watching setup and event processing
   - VCS status checking and cache updates

3. Include these structured fields:
   - `file.path` - file or directory path
   - `file.operation` - operation type (read, write, watch, etc.)
   - `file.size` - file size information
   - `fs.event.type` - file system event type
   - `vcs.status` - version control status
   - `tree.depth` - directory tree depth

4. Instrument async file operations:
   - Background VCS status updates
   - Asynchronous directory traversal
   - File content loading and caching

5. Add error handling for file operations:
   - Permission errors with suggested solutions
   - File not found errors with context
   - File system watcher errors with recovery actions

6. Track performance metrics:
   - Directory scan timing
   - File system event processing latency
   - VCS status check duration

7. Test file operation instrumentation:
   - Verify file operations produce structured logs
   - Check file watching events are properly traced
   - Test error scenarios and recovery logging
   - Ensure performance impact is acceptable for large directories
```

### Step 12: Performance Monitoring and Metrics

```
Implement performance monitoring and metrics collection:

1. Create performance monitoring utilities in `nucleotide-logging`:
   - Timing utilities for critical operations
   - Memory usage tracking helpers
   - Performance threshold alerting

2. Add performance spans to critical paths:
   - Application startup timing
   - Document loading and rendering
   - LSP request/response cycles
   - UI rendering and input processing

3. Implement metrics collection:
   - Operation counts and frequencies
   - Error rates and types
   - Resource usage patterns
   - User interaction patterns

4. Add performance-focused structured fields:
   - `perf.duration_ms` - operation timing
   - `perf.memory_mb` - memory usage
   - `perf.cpu_percent` - CPU utilization
   - `perf.fps` - UI frame rate
   - `perf.latency_ms` - operation latency

5. Create performance alerting:
   - Slow operation detection and logging
   - Memory leak detection
   - High error rate alerting

6. Implement performance profiling support:
   - Conditional detailed tracing for performance analysis
   - Sampling-based performance data collection
   - Integration with external profiling tools

7. Test performance monitoring:
   - Verify performance metrics are collected accurately
   - Check alerting works for slow operations
   - Test minimal overhead of performance monitoring
   - Validate usefulness for performance optimization
```

## Phase 4: Advanced Features

### Step 13: Log-based Debugging and Diagnostics

```
Implement advanced debugging features using the tracing infrastructure:

1. Create debugging utilities in `nucleotide-logging`:
   - Log analysis tools for common issues
   - Structured query helpers for filtering logs
   - Debug information collection for support

2. Add debug-specific tracing:
   - Detailed state dumps during error conditions
   - Step-by-step operation tracking for complex workflows
   - Memory and resource usage snapshots

3. Implement diagnostic commands:
   - Runtime log level adjustment
   - Component-specific debug mode enabling
   - State inspection and reporting

4. Create troubleshooting aids:
   - Common issue detection and reporting
   - Performance bottleneck identification
   - Configuration problem diagnosis

5. Add developer debugging features:
   - Enhanced trace output for development builds
   - Integration with debugging tools
   - Trace-based unit test helpers

6. Test debugging features:
   - Verify diagnostic information is useful
   - Check runtime debugging controls work correctly
   - Test troubleshooting aids with real issues
   - Ensure debugging overhead is acceptable
```

### Step 14: Configuration Management and Runtime Tuning

```
Implement comprehensive configuration management for the logging system:

1. Extend configuration options:
   - Per-component log level configuration
   - Output format customization
   - File rotation and retention policies
   - Performance tuning parameters

2. Add runtime configuration changes:
   - Hot-reload of logging configuration
   - Runtime log level adjustment
   - Dynamic output target modification

3. Create configuration validation:
   - Schema validation for configuration files
   - Error reporting for invalid configurations
   - Default fallback behavior

4. Implement configuration UI integration:
   - Settings panel for logging preferences
   - Runtime debugging controls
   - Log file access and viewing

5. Add configuration documentation:
   - Complete configuration reference
   - Best practices guide
   - Troubleshooting common configuration issues

6. Test configuration management:
   - Verify all configuration options work correctly
   - Check runtime changes take effect properly
   - Test error handling for invalid configurations
   - Ensure configuration persists correctly
```

### Step 15: Development and Production Profiles

```
Create optimized logging profiles for different usage scenarios:

1. Define logging profiles:
   - Development: verbose, pretty-printed, console-focused
   - Production: structured, file-based, performance-optimized
   - Debug: maximum verbosity, detailed context, all targets
   - Minimal: errors only, lightweight, essential info

2. Implement profile switching:
   - Environment-based profile selection
   - Command-line profile override
   - Runtime profile switching

3. Optimize for each profile:
   - Performance tuning for production profile
   - Readability optimization for development profile
   - Comprehensive coverage for debug profile
   - Minimal overhead for minimal profile

4. Create profile documentation:
   - When to use each profile
   - Profile-specific configuration options
   - Performance characteristics

5. Add profile management tools:
   - Profile validation and testing
   - Profile comparison utilities
   - Profile recommendation system

6. Test all profiles:
   - Verify each profile works as intended
   - Check performance characteristics
   - Test profile switching functionality
   - Validate documentation accuracy
```

### Step 16: Documentation and Best Practices

```
Create comprehensive documentation and establish best practices:

1. Write user documentation:
   - Quick start guide for developers
   - Complete API reference
   - Configuration guide with examples
   - Troubleshooting and FAQ

2. Create developer guidelines:
   - How to add new instrumentation
   - Structured field naming conventions
   - Performance considerations
   - Error handling patterns

3. Document architecture and design:
   - System architecture overview
   - Component interaction diagrams
   - Data flow documentation
   - Extension points and customization

4. Create examples and tutorials:
   - Common logging patterns
   - Advanced debugging techniques
   - Integration with external tools
   - Custom layer development

5. Establish maintenance procedures:
   - Log file management and cleanup
   - Configuration backup and recovery
   - Performance monitoring and optimization
   - Regular system health checks

6. Final testing and validation:
   - Complete system integration testing
   - Performance benchmarking
   - User acceptance testing
   - Documentation accuracy verification
   - Production readiness checklist completion
```

---

*Each prompt builds incrementally on the previous work, ensuring a systematic and thorough implementation of the comprehensive logging solution.*