# Nucleotide Comprehensive Logging Solution Plan

## Overview

This plan addresses the persistent logging issues in Nucleotide by implementing a comprehensive tokio-tracing based solution. The current codebase uses inconsistent logging with basic `log` crate macros scattered throughout, making debugging and observability difficult.

## Current State Analysis

### Existing Logging Issues
- **Inconsistent logging**: Mix of `log::debug!`, `log::info!`, `log::warn!`, `log::error!` without structured context
- **Basic setup**: Using `fern` for basic file/stdout logging in `main.rs:27-50`
- **No structured context**: No span tracking for async operations or request correlation
- **Limited observability**: No structured data or advanced filtering
- **Multi-crate complexity**: 7 separate crates with independent logging needs

### Architecture Context
- **Multi-crate workspace**: 7 crates (nucleotide-{core,events,types,ui,editor,workspace,lsp} + main)
- **Async runtime**: Full Tokio integration with GPUI event loop
- **Event-driven**: Complex event bridge system between Helix and GPUI
- **Modal editor**: Helix integration with complex state synchronization

## Proposed Architecture

### Core Components

1. **Centralized Logging Configuration** (`nucleotide-logging` crate)
   - Single source of truth for all logging configuration
   - Environment-based configuration (`NUCLEOTIDE_LOG`, `RUST_LOG` compatibility)
   - Multiple output targets (console, `~/.config/nucleotide/nucleotide.log`, structured JSON)

2. **Structured Tracing Infrastructure**
   - Replace all `log::*` macros with `tracing::*` equivalents
   - Add contextual spans for major operations (file ops, LSP calls, event handling)
   - Structured fields for better filtering and analysis

3. **Layered Subscriber Architecture**
   - Console output layer for development
   - File output layer with rotation to `~/.config/nucleotide/nucleotide.log`
   - Structured JSON layer for production monitoring
   - Performance-aware filtering

4. **Domain-Specific Instrumentation**
   - Editor operations (document changes, cursor movement, selections)
   - LSP interactions (requests/responses, server lifecycle)
   - File system operations (watching, reading, writing)
   - Event bridge operations (GPUI ↔ Helix communication)
   - UI rendering and interaction tracking

### Key Benefits
- **Contextual debugging**: Trace operations across async boundaries
- **Performance insights**: Built-in timing and performance monitoring
- **Structured queries**: Filter logs by operation type, document, user action
- **Development efficiency**: Clear operation flows and error tracking

## Implementation Strategy

### Phase 1: Foundation (Steps 1-4)
1. Create `nucleotide-logging` crate with tracing infrastructure
2. Implement configurable subscriber with multiple layers
3. Update workspace dependencies to use tracing
4. Basic smoke test and integration verification

### Phase 2: Core Migration (Steps 5-8)  
5. Migrate `nucleotide-core` event bridges with structured spans
6. Add instrumentation to document and editor operations
7. Instrument LSP communication and lifecycle
8. Update error handling with structured context

### Phase 3: Application Integration (Steps 9-12)
9. Integrate logging into main application startup
10. Add UI and workspace operation tracing
11. Instrument file operations and watching
12. Add performance monitoring and metrics

### Phase 4: Advanced Features (Steps 13-16)
13. Implement log-based debugging and diagnostics
14. Add configuration management and runtime tuning
15. Create development and production logging profiles
16. Document usage patterns and best practices

## Success Criteria

### Technical Objectives
- [ ] Zero `log::*` macro usage (replaced with `tracing::*`)
- [ ] Contextual spans for all major async operations
- [ ] Structured logging with searchable fields
- [ ] Configurable output targets (console, `~/.config/nucleotide/nucleotide.log`, JSON)
- [ ] Performance overhead < 5% in release builds
- [ ] Compatible with existing Helix logging patterns

### Development Experience
- [ ] Clear operation traces for debugging complex interactions
- [ ] Filterable logs by component, operation type, or document
- [ ] Readable console output for development
- [ ] Structured JSON output for production analysis
- [ ] Documentation and examples for adding new instrumentation

### Production Readiness
- [ ] Log rotation and size management for `~/.config/nucleotide/nucleotide.log`
- [ ] Performance monitoring capabilities
- [ ] Security: no credential or sensitive data leakage
- [ ] Configurable verbosity levels per component
- [ ] Integration tests covering logging behavior

## Risk Mitigation

### Performance Concerns
- Use conditional compilation for expensive operations
- Implement proper filtering to minimize overhead
- Benchmark before/after to ensure acceptable performance

### Migration Complexity
- Incremental migration approach (crate by crate)
- Maintain backwards compatibility during transition
- Comprehensive testing at each step

### Helix Integration
- Preserve existing Helix logging behavior where possible
- Use feature flags for optional enhanced instrumentation
- Ensure no conflicts with Helix's own logging infrastructure

---

*This plan ensures a robust, observable, and maintainable logging solution that will significantly improve debugging capabilities and production monitoring for Nucleotide.*