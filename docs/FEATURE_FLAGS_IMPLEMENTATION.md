# Feature Flag System Implementation

## Overview

This document describes the implementation of the feature flag system for project-based LSP startup in Nucleotide, with comprehensive fallback mechanisms and runtime configuration support.

## Architecture

### Core Components

1. **Configuration System** (`src/config.rs`)
   - `LspConfig` struct with feature flag settings
   - Validation and sanitization logic
   - Runtime configuration reloading

2. **LSP Manager** (`src/lsp_manager.rs`)
   - Centralized LSP startup management
   - Feature flag-driven startup mode determination
   - Comprehensive error handling and fallback mechanisms

3. **Application Integration** (`src/application.rs` & `src/workspace.rs`)
   - Integration points for LSP startup
   - Configuration hot-reloading support
   - Event-driven LSP initialization

## Feature Flags

### `project_lsp_startup`
- **Type**: Boolean
- **Default**: `false`
- **Purpose**: Enable project-based LSP startup vs file-based startup
- **Behavior**: 
  - When `true`: LSP servers start when project is detected
  - When `false`: LSP servers start when files are opened (existing behavior)

### `startup_timeout_ms`
- **Type**: u64
- **Default**: `5000` (5 seconds)
- **Range**: 1-60000 milliseconds
- **Purpose**: Timeout for LSP server initialization
- **Validation**: Automatically sanitized to valid range

### `enable_fallback`
- **Type**: Boolean
- **Default**: `true`
- **Purpose**: Enable graceful fallback to file-based startup when project detection fails
- **Behavior**: Ensures LSP always works even if project detection fails

## Configuration Example

```toml
# ~/.config/helix/nucleotide.toml

[lsp]
# Enable project-based LSP startup
project_lsp_startup = true

# LSP startup timeout (5 seconds)
startup_timeout_ms = 5000

# Enable fallback to file-based startup
enable_fallback = true
```

## Startup Modes

### Project-Based Mode
- **Trigger**: Project root detected (VCS directories: .git, .svn, .hg, .jj, .helix)
- **Behavior**: LSP servers initialize for the entire project context
- **Advantages**: Better project-wide features, faster subsequent file opens
- **Fallback**: Falls back to file-based mode if project detection fails

### File-Based Mode (Existing Behavior)
- **Trigger**: Individual files opened
- **Behavior**: LSP servers initialize per file
- **Advantages**: Always works, no project dependency
- **Use Cases**: Single files, non-project contexts

## Error Handling

### Error Types
- `DocumentNotFound`: Document ID not found in editor
- `NoLanguageConfig`: Document has no language configuration
- `ProjectDetectionFailed`: Unable to detect project root
- `StartupTimeout`: LSP initialization exceeded timeout
- `ConfigValidationFailed`: Invalid configuration values
- `FallbackFailed`: Both primary and fallback modes failed
- `CommunicationError`: LSP communication issues

### Fallback Mechanism
1. **Primary Mode Fails**: Try primary startup mode (project or file)
2. **Fallback Enabled**: If primary fails and fallback enabled, try file-based mode
3. **Graceful Degradation**: Continue with existing LSP behavior as last resort
4. **Error Reporting**: Log detailed error information for debugging

## Runtime Configuration Changes

### Hot-Reloading Support
- Configuration changes are applied immediately without restart
- LSP manager validates new configuration before applying
- Invalid configurations are rejected with error logging
- Previous valid configuration is preserved on validation failure

### Configuration Events
- `:config-reload` command triggers configuration refresh
- `:set` commands trigger configuration updates
- File-based configuration changes are detected and applied

## Integration Points

### Document Opening
- **File Picker**: Uses feature flag system when files are opened
- **Command Line**: Initial file arguments processed with feature flags
- **Helix Events**: `DocumentOpened` events trigger LSP startup

### Configuration Updates
- **Real-time**: Configuration changes update LSP manager immediately
- **Validation**: New configurations are validated before application
- **Error Handling**: Invalid configurations maintain previous settings

## Logging and Monitoring

### Structured Logging
- All LSP operations use structured logging with context
- Startup mode, timing, and error information captured
- Performance metrics for startup duration
- Feature flag state logged on configuration changes

### Debug Information
- LSP startup attempts tracked with statistics
- Project detection results logged
- Fallback mechanism usage monitored
- Configuration validation results recorded

## Testing

### Unit Tests
- Configuration parsing and validation
- LSP manager startup mode determination
- Error handling and fallback mechanisms
- Configuration sanitization

### Integration Tests
- Feature flag system with real LSP servers
- Runtime configuration changes
- Project detection accuracy
- Fallback behavior verification

## Backward Compatibility

### Seamless Fallback
- Feature disabled by default maintains existing behavior
- All existing LSP functionality preserved
- No breaking changes to existing configurations
- Graceful degradation when new features fail

### Migration Path
- Users can enable features incrementally
- Configuration validation prevents invalid states
- Clear error messages guide configuration fixes
- Example configurations provided for common use cases

## Performance Considerations

### Startup Optimization
- Project-based mode reduces repeated LSP initialization
- Caching of project detection results
- Timeout controls prevent indefinite hangs
- Fallback mechanisms ensure quick recovery

### Resource Management
- LSP manager tracks startup attempts to prevent resource leaks
- Statistics collection for performance monitoring
- Error handling prevents cascading failures
- Memory-efficient startup mode determination

## Future Enhancements

### Potential Extensions
- Per-language server timeout configuration
- Advanced project detection rules
- LSP server lifecycle management
- Performance-based automatic mode switching

### Monitoring Integration
- LSP startup success/failure metrics
- Performance tracking and alerting
- Configuration validation statistics
- User experience analytics