# Nucleotide Project Detection and Configuration Test Suite

This directory contains comprehensive tests for the project detection and configuration system in Nucleotide. The test suite ensures robust project identification, configuration parsing, and error handling across various scenarios.

## Test Organization

### 1. Project Detection Tests (`project_detection_tests.rs`)

Tests the workspace root detection algorithm that identifies project boundaries using VCS directories.

**Key Test Areas:**
- **VCS Detection**: Tests detection of `.git`, `.svn`, `.hg`, `.jj`, and `.helix` directories
- **Nested Projects**: Verifies handling of nested repositories and project structures
- **Complex Hierarchies**: Tests deep directory structures and monorepos
- **Manifest Detection**: Tests identification of project types via `Cargo.toml`, `package.json`, etc.
- **ProjectDetector Trait**: Tests abstract project detection interface implementations
- **ManifestProvider Trait**: Tests manifest parsing for different project types

**Test Fixtures:**
- Creates temporary directories with realistic project structures
- Supports Rust workspaces, Node.js monorepos, Python projects
- Mock file system for testing without actual I/O

### 2. Configuration System Tests (`config_system_tests.rs`)

Tests configuration parsing, validation, and merging between GUI and Helix configurations.

**Key Test Areas:**
- **GUI Config Parsing**: Tests `nucleotide.toml` parsing and validation
- **Helix Config Integration**: Tests merging with `config.toml` 
- **Font Configuration**: Tests font family, size, weight parsing
- **Theme Configuration**: Tests theme mode and theme selection
- **Validation**: Tests configuration validation and error handling
- **Migration**: Tests backward compatibility and config schema evolution

**Configuration Types Tested:**
- UI fonts and editor fonts with fallback chains
- Theme modes (system, light, dark)
- Window appearance settings
- Invalid configuration recovery

### 3. Integration Tests (`integration_tests.rs`)

End-to-end tests that simulate real-world project scenarios and workflows.

**Key Test Areas:**
- **Complex Project Structures**: Tests realistic monorepos and mixed-language projects
- **Configuration Loading**: Tests complete config loading workflows
- **Error Recovery**: Tests graceful handling of corrupted or missing files
- **VCS Priority**: Tests nested VCS directory handling
- **Performance**: Tests detection speed with large project structures

**Realistic Scenarios:**
- Rust workspaces with multiple crates
- Node.js monorepos with multiple packages  
- Python projects with various manifest formats
- Mixed-language projects with multiple build systems
- Projects with documentation, tools, and scripts

### 4. Performance Tests (`performance_tests.rs`)

Benchmark tests to ensure project detection and configuration loading remain fast and scalable.

**Key Test Areas:**
- **Depth Performance**: Tests workspace detection with deeply nested directories
- **Breadth Performance**: Tests detection with wide directory structures
- **Config Loading Speed**: Tests configuration parsing performance
- **Concurrent Access**: Tests thread safety and concurrent detection
- **Memory Usage**: Tests for memory leaks and excessive allocation
- **Scalability Limits**: Tests extreme cases and edge conditions

**Performance Targets:**
- Workspace detection: < 10ms for 100+ directory levels
- Config loading: < 50ms for 100KB configuration files
- Repeated detection: < 1ms average, < 5ms variance
- Concurrent detection: < 1 second for 10 threads × 100 operations

## Test Infrastructure

### Test Utilities

- **ProjectStructureBuilder**: Creates realistic project structures in temporary directories
- **ConfigTestBuilder**: Creates test configuration files with various content
- **MockFileSystem**: Simulates file system operations without actual I/O
- **BenchmarkResult**: Measures and reports performance metrics
- **PerformanceTestSuite**: Utilities for creating performance test scenarios

### Supported Project Types

The test suite validates detection and configuration for:

- **Rust**: `Cargo.toml` workspaces and individual crates
- **Node.js**: `package.json` with npm/yarn workspaces  
- **Python**: `pyproject.toml`, `setup.py`, `requirements.txt`
- **Generic**: Any project with VCS directories
- **Mixed**: Projects combining multiple languages/build systems

### VCS Support

Tests cover all supported version control systems:

- Git (`.git`)
- Subversion (`.svn`)
- Mercurial (`.hg`) 
- Jujutsu (`.jj`)
- Helix project marker (`.helix`)

## Running Tests

### Full Test Suite
```bash
cargo test --package nucleotide tests
```

### Specific Test Modules
```bash
# Project detection tests
cargo test --package nucleotide tests::project_detection_tests

# Configuration tests  
cargo test --package nucleotide tests::config_system_tests

# Integration tests
cargo test --package nucleotide tests::integration_tests

# Performance benchmarks
cargo test --package nucleotide tests::performance_tests
```

### Test Categories
```bash
# Unit tests only
cargo test --package nucleotide tests::project_detection_tests::tests
cargo test --package nucleotide tests::config_system_tests::tests

# Integration tests only  
cargo test --package nucleotide tests::integration_tests::tests

# Performance tests only
cargo test --package nucleotide tests::performance_tests::tests
```

## Test Coverage

The test suite provides comprehensive coverage of:

### Edge Cases
- ✅ Non-existent paths and permission errors
- ✅ Corrupted configuration files
- ✅ Missing manifest files
- ✅ Nested and conflicting VCS directories
- ✅ Very deep directory structures
- ✅ Projects without VCS markers
- ✅ Symlinks and filesystem edge cases

### Error Conditions
- ✅ Malformed TOML/JSON configuration
- ✅ Invalid font specifications
- ✅ Missing required configuration sections
- ✅ File system access errors
- ✅ Circular directory structures
- ✅ Permission denied scenarios

### Performance Scenarios  
- ✅ Large project hierarchies (1000+ directories)
- ✅ Deep nesting (200+ levels)
- ✅ Large configuration files (100KB+)
- ✅ Concurrent access patterns
- ✅ Memory usage patterns
- ✅ Repeated operations

## Adding New Tests

### For Project Detection
1. Add test cases to `project_detection_tests.rs`
2. Use `TestProject` helper for creating project structures
3. Test both positive and negative cases
4. Include performance considerations for new detection logic

### For Configuration  
1. Add test cases to `config_system_tests.rs`
2. Use `ConfigTestBuilder` for creating test configurations
3. Test both valid and invalid configuration scenarios
4. Include validation and migration tests

### For Integration Scenarios
1. Add test cases to `integration_tests.rs`  
2. Create realistic project structures
3. Test complete workflows end-to-end
4. Include error recovery and edge cases

### For Performance
1. Add benchmarks to `performance_tests.rs`
2. Use `benchmark()` utility for measuring performance
3. Set appropriate performance targets
4. Test scalability and resource usage

## Dependencies

The test suite requires these dependencies:

- `tempfile`: For creating temporary test directories
- `toml`: For parsing TOML configuration files
- `serde_json`: For parsing JSON manifest files
- `nucleotide_logging`: For structured test logging

These are included in the main crate's `Cargo.toml` under `[dev-dependencies]`.

## Test Philosophy

The test suite follows these principles:

1. **Comprehensive Coverage**: Tests cover all supported project types, configurations, and edge cases
2. **Realistic Scenarios**: Tests use realistic project structures and configurations
3. **Performance Validation**: Tests ensure acceptable performance under various loads
4. **Error Resilience**: Tests verify graceful handling of error conditions
5. **Future-Proofing**: Tests validate backward compatibility and schema evolution

## Maintenance

### Regular Maintenance Tasks
- Run full test suite with each code change
- Update performance targets as the codebase grows
- Add tests for new project types and configuration options
- Review and update realistic test scenarios
- Monitor test execution time and optimize slow tests

### Performance Monitoring
- Performance tests should be run regularly to detect regressions
- Benchmark results are logged for tracking trends
- Failed performance tests indicate potential optimization needs
- New features should include performance tests

This test suite ensures that Nucleotide's project detection and configuration system remains robust, fast, and reliable across a wide variety of real-world scenarios.