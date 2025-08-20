# LSP Server Lifecycle Management - Comprehensive Test Suite

This document describes the comprehensive integration test suite for the LSP server lifecycle management system in Nucleotide.

## Overview

The test suite provides end-to-end validation of the project-based LSP system, including edge cases, error conditions, and performance characteristics. The tests are organized into three main modules:

1. **`integration_tests.rs`** - Core lifecycle integration tests
2. **`mock_server_tests.rs`** - Mock LSP server implementations and unit tests
3. **`stress_tests.rs`** - Performance, concurrency, and edge case tests

## Test Architecture

### Mock Infrastructure

The test suite uses a sophisticated mock LSP server infrastructure that provides:

- **Controllable server behavior** - Configure startup delays, failure modes, resource usage
- **Health monitoring simulation** - Intermittent failures, performance metrics
- **Event tracking** - Request counting, response times, lifecycle events
- **Registry management** - Server registration, lookup, and cleanup

### Test Categories

#### 1. Integration Tests (`integration_tests.rs`)

**Core Lifecycle Tests:**
- `test_complete_server_lifecycle` - End-to-end server startup, operation, shutdown
- `test_project_detection_triggering_server_startup` - Project detection → server startup flow
- `test_fallback_to_file_based_lsp` - Behavior when project detection fails
- `test_multiple_language_servers_same_project` - Mixed-language project handling
- `test_server_cleanup_and_resource_management` - Proper resource cleanup
- `test_concurrent_lsp_server_operations` - Concurrent project detection
- `test_performance_validation` - Response time and throughput validation
- `test_error_recovery_scenarios` - Error handling and recovery
- `test_server_health_monitoring` - Health check system validation
- `test_project_type_detection_accuracy` - Project type detection accuracy
- `test_server_lifecycle_with_editor_integration` - Editor integration interface

**Key Features Tested:**
- Proactive server startup on project detection
- Fallback to file-based LSP when project detection fails
- Multiple language servers per project
- Server health monitoring and status tracking
- Resource cleanup and lifecycle management
- Error recovery and timeout handling
- Performance characteristics under normal load

#### 2. Mock Server Tests (`mock_server_tests.rs`)

**Mock Server Infrastructure:**
- `MockLspServer` - Configurable mock LSP server implementation
- `MockServerBehavior` - Behavior configuration (delays, failures, resource usage)
- `MockServerRegistry` - Server registration and lifecycle management
- `MockLspRequest/Response` - Request/response simulation

**Mock Server Test Cases:**
- Server creation and startup sequences
- Failure scenario simulation (startup failures, timeouts)
- Request handling and response generation
- Health check simulation with intermittent failures
- Resource usage simulation and monitoring
- Registry operations (register, unregister, lookup)
- Performance characteristics measurement

**Controllable Behaviors:**
- Startup delays (simulate slow server initialization)
- Startup failures (configuration errors, missing dependencies)
- Response delays (simulate slow server responses)
- Intermittent health failures (simulate server instability)
- High resource usage (memory/CPU simulation)
- Request counting and metrics collection

#### 3. Stress Tests (`stress_tests.rs`)

**High-Load Scenarios:**
- `test_concurrent_project_detection` - Many projects detected simultaneously
- `test_high_frequency_health_checks` - Rapid health check operations
- `test_server_startup_timeout_handling` - Timeout behavior under load
- `test_memory_pressure_simulation` - High memory usage scenarios
- `test_rapid_project_creation_and_deletion` - Rapid lifecycle operations
- `test_edge_case_project_structures` - Unusual project configurations
- `test_concurrent_server_lifecycle_operations` - Concurrent startup/shutdown

**Performance Validation:**
- Response time measurement and validation
- Throughput testing under various loads
- Resource usage monitoring
- Concurrency limit enforcement
- Timeout handling accuracy
- Error rate measurement

**Edge Cases:**
- Empty directories
- Ambiguous project types (multiple project files)
- Very long path names
- Hidden files only
- Nested structures without clear project markers
- Invalid file permissions
- Corrupted project files

## Test Configuration

### Environment Variables

- `NUCLEOTIDE_LOG` - Set log level for test runs
- `RUST_TEST_THREADS` - Control test parallelism
- `RUST_BACKTRACE` - Enable backtraces for debugging

### Test Configuration Objects

```rust
// Integration test configuration
TestConfig {
    test_dir: PathBuf,           // Temporary directory for test projects
    operation_timeout: Duration, // Maximum time for operations
    server_startup_delay: Duration, // Mock server startup delay
}

// Stress test configuration
StressTestConfig {
    concurrent_projects: usize,     // Number of simultaneous projects
    servers_per_project: usize,     // Servers per project
    test_duration: Duration,        // How long to run stress tests
    operation_timeout: Duration,    // Individual operation timeout
    max_failure_rate: f32,         // Acceptable failure percentage
}

// Mock server behavior
MockServerBehavior {
    startup_delay: Duration,           // Simulate slow startup
    startup_failure: bool,             // Force startup failure
    response_delay: Duration,          // Response latency
    health_status: ServerHealthStatus, // Health check response
    intermittent_health_failure: bool, // Unstable health checks
    memory_usage_mb: u64,              // Simulated memory usage
    cpu_usage_percent: f32,            // Simulated CPU usage
}
```

## Running Tests

### Individual Test Categories

```bash
# Run integration tests
cargo test -p nucleotide-lsp integration_tests

# Run mock server tests
cargo test -p nucleotide-lsp mock_server_tests

# Run stress tests (may take several minutes)
cargo test -p nucleotide-lsp stress_tests
```

### Specific Test Cases

```bash
# Test project detection
cargo test -p nucleotide-lsp test_project_detection_triggering_server_startup

# Test concurrent operations
cargo test -p nucleotide-lsp test_concurrent_lsp_server_operations

# Test error recovery
cargo test -p nucleotide-lsp test_error_recovery_scenarios
```

### Performance Testing

```bash
# Run with performance metrics
NUCLEOTIDE_LOG=info cargo test -p nucleotide-lsp test_performance_validation

# Run stress tests with detailed logging
NUCLEOTIDE_LOG=debug cargo test -p nucleotide-lsp stress_tests
```

## Test Data and Cleanup

### Temporary Directories

Tests create temporary project structures in:
- **macOS/Linux**: `/tmp/nucleotide_*_tests/`
- **Windows**: `%TEMP%\nucleotide_*_tests\`

### Automatic Cleanup

- Test projects are automatically cleaned up after each test
- Failed tests may leave temporary directories for debugging
- Use `cargo clean` to ensure complete cleanup

### Manual Cleanup

```bash
# Remove all test directories
rm -rf /tmp/nucleotide_*_tests/

# On Windows:
# rmdir /s %TEMP%\nucleotide_*_tests\
```

## Performance Metrics

### Measured Characteristics

- **Project Detection Time**: < 100ms for typical projects
- **Server Startup Time**: < 10s including actual LSP server launch
- **Health Check Frequency**: Configurable, default 30s intervals
- **Concurrent Operations**: Up to configured `max_concurrent_startups`
- **Memory Usage**: Tracked per mock server
- **Failure Rates**: Acceptable failure rate < 5% under stress

### Performance Assertions

Tests include performance assertions to ensure:
- Response times remain within acceptable limits
- Memory usage doesn't grow unbounded
- Concurrent operations don't cause excessive delays
- Error rates stay within acceptable bounds
- Cleanup operations complete promptly

## Mock vs Real Integration

### Mock Server Benefits

- **Deterministic behavior** - Predictable responses for testing
- **Failure simulation** - Test error conditions safely
- **Performance control** - Configurable delays and resource usage
- **Isolated testing** - No external dependencies
- **Rapid execution** - Fast test cycles

### Real Integration Testing

While the comprehensive test suite uses mock servers for most scenarios, real integration testing should be performed with:

- Actual LSP servers (rust-analyzer, typescript-language-server, etc.)
- Real project structures
- Live editor integration
- Network conditions and latency
- Resource constraints

## Test Maintenance

### Adding New Tests

1. **Integration tests**: Add to `integration_tests.rs` for core functionality
2. **Mock server tests**: Add to `mock_server_tests.rs` for infrastructure testing
3. **Stress tests**: Add to `stress_tests.rs` for performance/edge cases

### Updating Mock Behavior

When LSP server behavior changes:
1. Update `MockServerBehavior` configuration
2. Add new request/response types to `MockLspRequest/Response`
3. Update health check logic in mock servers
4. Validate test assertions still align with expected behavior

### Performance Baseline Updates

As the system improves:
1. Review performance assertions in tests
2. Update acceptable response times
3. Adjust stress test parameters
4. Update failure rate thresholds

## Troubleshooting

### Common Test Failures

1. **Timeout failures**: Check system load, increase timeout values
2. **Directory creation failures**: Check permissions, disk space
3. **Event collection failures**: Verify event channel setup
4. **Performance assertion failures**: Review system performance, update baselines

### Debugging Tests

```bash
# Run with detailed logging
NUCLEOTIDE_LOG=debug RUST_BACKTRACE=1 cargo test -p nucleotide-lsp test_name

# Run single test with output
cargo test -p nucleotide-lsp test_name -- --nocapture

# Run tests serially to avoid resource conflicts
cargo test -p nucleotide-lsp -- --test-threads=1
```

### Test Environment Issues

- Ensure sufficient disk space for temporary directories
- Check file system permissions
- Verify network access not required for mock tests
- Consider system load when running stress tests

## Coverage and Quality

The test suite provides comprehensive coverage of:

- **Functional requirements**: All core LSP lifecycle operations
- **Error conditions**: Startup failures, timeouts, invalid configurations
- **Performance characteristics**: Response times, throughput, resource usage
- **Concurrency**: Multiple simultaneous operations
- **Edge cases**: Unusual project structures, system conditions
- **Integration points**: Event flow, cleanup operations

The tests serve as both validation and documentation of expected system behavior, providing confidence in the robustness and performance of the LSP server lifecycle management system.