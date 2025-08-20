# Project Markers Configuration Guide

## Overview

The `project_markers.toml` configuration system provides powerful project detection and LSP server management capabilities for Nucleotide. This system allows you to customize how projects are detected, which language servers are started, and how project roots are determined based on your specific needs.

## Configuration Location

Create your `project_markers.toml` file in your Helix configuration directory:

- **macOS**: `~/.config/helix/project_markers.toml`
- **Linux**: `~/.config/helix/project_markers.toml`
- **Windows**: `%APPDATA%\helix\project_markers.toml`

## Complete Schema Reference

### Basic Configuration Structure

```toml
# Project markers configuration
version = "1.0"

# Global settings
[global]
max_depth = 10                    # Maximum directory levels to search upward
cache_duration_minutes = 60       # Cache project detection results
enable_parallel_detection = true  # Enable parallel project detection
timeout_seconds = 5               # Timeout for project detection operations

# Default LSP settings that apply to all projects unless overridden
[global.lsp]
startup_timeout_ms = 5000        # Default LSP startup timeout
enable_health_checks = true     # Enable periodic LSP health monitoring
max_concurrent_servers = 5       # Maximum concurrent LSP server startups
auto_restart_failed = true      # Auto-restart crashed LSP servers
graceful_shutdown_timeout = 10   # Seconds to wait for graceful LSP shutdown

# Project type definitions
[[projects]]
name = "rust"
priority = 150                   # Higher priority = checked first
enabled = true                  # Enable/disable this project type

# File patterns that indicate this project type
markers = [
    "Cargo.toml",               # Primary marker (highest priority)
    "Cargo.lock",               # Secondary marker
]

# Additional validation patterns (all must be present)
required_patterns = []

# Patterns that exclude this project type (any present = not this type)
exclude_patterns = [
    ".no-rust",
    "package.json"  # Exclude if it looks like a JS project with Cargo.toml
]

# Root detection strategy
[projects.root_detection]
strategy = "outermost"          # "outermost", "innermost", "workspace_aware"
workspace_markers = ["Cargo.toml"]  # Files that indicate workspace roots
prefer_workspace = true         # Prefer workspace root over member roots

# LSP configuration for this project type
[projects.lsp]
servers = ["rust-analyzer"]     # Language servers to start
primary_language = "rust"       # Primary language identifier
startup_timeout_ms = 8000      # Override global timeout for this project
enable_completion_cache = true # Enable completion result caching
features = ["diagnostics", "hover", "completion", "references"]

# Server-specific configuration
[projects.lsp.server_config.rust-analyzer]
command = "rust-analyzer"       # Executable name or path
args = []                      # Command line arguments
env = {}                       # Environment variables
working_directory = "."        # Working directory relative to project root
initialization_options = {}    # LSP initialization options

# Health check configuration
[projects.lsp.health_check]
enabled = true                 # Enable health checks for this project
interval_seconds = 30         # Health check frequency
timeout_ms = 1000            # Health check timeout
failure_threshold = 3        # Failed checks before marking unhealthy

# File association patterns
[projects.file_patterns]
primary = ["*.rs"]            # Primary file extensions
secondary = ["*.toml"]        # Secondary file extensions
ignore = [                    # Patterns to ignore
    "target/**",
    "**/.git/**",
    "**/node_modules/**"
]
```

## Language-Specific Examples

### Rust Projects

```toml
[[projects]]
name = "rust"
priority = 150
enabled = true
markers = ["Cargo.toml"]
exclude_patterns = [".no-rust"]

[projects.root_detection]
strategy = "workspace_aware"
workspace_markers = ["Cargo.toml"]
prefer_workspace = true

[projects.lsp]
servers = ["rust-analyzer"]
primary_language = "rust"
startup_timeout_ms = 8000
features = ["diagnostics", "hover", "completion", "references", "rename"]

[projects.lsp.server_config.rust-analyzer]
command = "rust-analyzer"
initialization_options = { "cargo" = { "buildScripts" = { "enable" = true } } }

[projects.file_patterns]
primary = ["*.rs"]
secondary = ["*.toml", "*.md"]
ignore = ["target/**", "**/target/**"]
```

### TypeScript/JavaScript Projects

```toml
[[projects]]
name = "typescript"
priority = 140
enabled = true
markers = ["tsconfig.json", "package.json"]
required_patterns = ["tsconfig.json"]

[projects.root_detection]
strategy = "innermost"  # TS projects often have nested configs

[projects.lsp]
servers = ["typescript-language-server", "eslint"]
primary_language = "typescript"
startup_timeout_ms = 6000

[projects.lsp.server_config.typescript-language-server]
command = "typescript-language-server"
args = ["--stdio"]
initialization_options = {
    "preferences" = {
        "includeCompletionsForModuleExports" = true,
        "includeCompletionsForImportStatements" = true
    }
}

[projects.lsp.server_config.eslint]
command = "vscode-eslint-language-server"
args = ["--stdio"]

[projects.file_patterns]
primary = ["*.ts", "*.tsx"]
secondary = ["*.js", "*.jsx", "*.json"]
ignore = ["node_modules/**", "dist/**", "build/**"]

# JavaScript-only projects
[[projects]]
name = "javascript"
priority = 130
enabled = true
markers = ["package.json"]
exclude_patterns = ["tsconfig.json", "*.ts", "*.tsx"]

[projects.lsp]
servers = ["typescript-language-server"]
primary_language = "javascript"

[projects.file_patterns]
primary = ["*.js", "*.jsx"]
secondary = ["*.json", "*.md"]
```

### Python Projects

```toml
[[projects]]
name = "python"
priority = 140
enabled = true
markers = [
    "pyproject.toml",      # Modern Python projects
    "setup.py",            # Traditional setup
    "requirements.txt",    # Pip requirements
    "Pipfile",            # Pipenv
    "poetry.lock",        # Poetry
    "environment.yml"     # Conda
]

[projects.root_detection]
strategy = "outermost"
workspace_markers = ["pyproject.toml", "setup.py"]

[projects.lsp]
servers = ["pyright", "ruff-lsp"]
primary_language = "python"
startup_timeout_ms = 7000

[projects.lsp.server_config.pyright]
command = "pyright-langserver"
args = ["--stdio"]
initialization_options = {
    "settings" = {
        "python" = {
            "analysis" = {
                "typeCheckingMode" = "basic",
                "autoImportCompletions" = true
            }
        }
    }
}

[projects.lsp.server_config.ruff-lsp]
command = "ruff-lsp"
args = []

[projects.file_patterns]
primary = ["*.py", "*.pyi"]
secondary = ["*.toml", "*.txt", "*.yml", "*.yaml"]
ignore = [
    "__pycache__/**",
    "*.pyc",
    ".pytest_cache/**",
    "venv/**",
    ".venv/**",
    "env/**"
]
```

### Go Projects

```toml
[[projects]]
name = "go"
priority = 140
enabled = true
markers = ["go.mod"]
required_patterns = ["go.mod"]

[projects.root_detection]
strategy = "innermost"  # Go modules are typically self-contained

[projects.lsp]
servers = ["gopls"]
primary_language = "go"
startup_timeout_ms = 5000

[projects.lsp.server_config.gopls]
command = "gopls"
args = []
initialization_options = {
    "gofumpt" = true,
    "staticcheck" = true,
    "vulncheck" = "Imports"
}

[projects.file_patterns]
primary = ["*.go"]
secondary = ["*.mod", "*.sum"]
ignore = ["vendor/**"]
```

### Java Projects

```toml
[[projects]]
name = "java-maven"
priority = 145
enabled = true
markers = ["pom.xml"]
required_patterns = ["pom.xml"]

[projects.root_detection]
strategy = "outermost"  # Maven can have parent POMs

[projects.lsp]
servers = ["jdtls"]
primary_language = "java"
startup_timeout_ms = 15000  # Java LSP takes longer to start

[projects.lsp.server_config.jdtls]
command = "jdtls"
args = []
working_directory = "."

[projects.file_patterns]
primary = ["*.java"]
secondary = ["*.xml", "*.properties"]
ignore = ["target/**", ".m2/**"]

# Gradle projects
[[projects]]
name = "java-gradle"
priority = 144
enabled = true
markers = ["build.gradle", "build.gradle.kts", "gradlew"]

[projects.lsp]
servers = ["jdtls"]
primary_language = "java"

[projects.file_patterns]
primary = ["*.java", "*.kt"]  # Also support Kotlin
secondary = ["*.gradle", "*.kts", "*.properties"]
ignore = ["build/**", ".gradle/**"]
```

### C/C++ Projects

```toml
[[projects]]
name = "cpp-cmake"
priority = 140
enabled = true
markers = ["CMakeLists.txt"]

[projects.root_detection]
strategy = "outermost"

[projects.lsp]
servers = ["clangd"]
primary_language = "cpp"
startup_timeout_ms = 6000

[projects.lsp.server_config.clangd]
command = "clangd"
args = ["--background-index", "--clang-tidy"]

[projects.file_patterns]
primary = ["*.cpp", "*.cc", "*.cxx", "*.c", "*.h", "*.hpp"]
secondary = ["*.cmake", "CMakeLists.txt"]
ignore = ["build/**", "cmake-build-*/**"]

# Makefile-based projects
[[projects]]
name = "c-make"
priority = 130
enabled = true
markers = ["Makefile", "makefile"]
exclude_patterns = ["CMakeLists.txt"]  # Prefer CMake over Make

[projects.lsp]
servers = ["clangd"]
primary_language = "c"

[projects.file_patterns]
primary = ["*.c", "*.h"]
secondary = ["Makefile", "makefile", "*.mk"]
```

## Root Detection Strategies

### Outermost Strategy

Searches from the current file upward and returns the highest-level directory containing project markers. Best for:

- Workspace-based projects (Rust workspaces, Maven multi-module)
- Monorepos with multiple sub-projects
- Projects with nested sub-components

```toml
[projects.root_detection]
strategy = "outermost"
workspace_markers = ["Cargo.toml", "pom.xml"]
prefer_workspace = true
```

### Innermost Strategy  

Returns the first (closest to file) directory containing project markers. Best for:

- Self-contained projects
- Language-specific projects with clear boundaries
- Projects where each subdirectory is independent

```toml
[projects.root_detection]
strategy = "innermost"
```

### Workspace-Aware Strategy

Intelligently detects workspace vs. member projects and prefers workspace roots. Best for:

- Languages with explicit workspace concepts
- Mixed project structures
- Complex project hierarchies

```toml
[projects.root_detection]
strategy = "workspace_aware"
workspace_markers = ["Cargo.toml", "lerna.json"]
prefer_workspace = true
workspace_indicators = [
    { pattern = "Cargo.toml", content_contains = ["[workspace]"] },
    { pattern = "package.json", content_contains = ["\"workspaces\""] }
]
```

## Priority System

Projects are evaluated in priority order (highest first). This ensures more specific project types are detected before generic ones:

- **150+**: Language-specific projects with strong indicators (Rust with Cargo.toml)
- **140-149**: Well-defined projects with clear markers (Python with pyproject.toml)  
- **130-139**: Generic or legacy project formats (JavaScript with package.json only)
- **120-129**: Fallback detectors
- **< 120**: Experimental or specialized detectors

```toml
[[projects]]
name = "rust-workspace"
priority = 160  # Highest priority for Rust workspaces

[[projects]] 
name = "rust-member"
priority = 150  # Lower priority for member crates

[[projects]]
name = "generic-toml"
priority = 100  # Lowest priority fallback
```

## Performance Optimization

### Caching

```toml
[global]
# Cache project detection results to avoid repeated filesystem scans
cache_duration_minutes = 60
cache_max_entries = 1000

# Invalidate cache when these files change
cache_invalidation_patterns = [
    "Cargo.toml",
    "package.json", 
    "pyproject.toml",
    ".gitignore"
]
```

### Parallel Detection

```toml
[global]
enable_parallel_detection = true
max_parallel_detectors = 4      # Number of concurrent detectors
detection_timeout_seconds = 5   # Timeout for entire detection process
```

### Exclusion Patterns

Use exclusion patterns to avoid scanning irrelevant directories:

```toml
[global]
# Global exclusions applied to all projects
global_exclude_patterns = [
    "**/node_modules/**",
    "**/target/**", 
    "**/.git/**",
    "**/build/**",
    "**/dist/**",
    "**/.pytest_cache/**",
    "**/__pycache__/**"
]

# Maximum depth to search (prevents excessive scanning)
max_depth = 10
```

## Integration with Nucleotide Configuration

### LSP Feature Flags Integration

The project markers system integrates seamlessly with nucleotide.toml LSP configuration:

**nucleotide.toml**:
```toml
[lsp]
# Enable project-based LSP startup
project_lsp_startup = true
startup_timeout_ms = 5000
enable_fallback = true

# Use project markers for detection
use_project_markers = true
project_markers_file = "project_markers.toml"  # Optional, defaults to this name
```

**project_markers.toml** values override **nucleotide.toml** defaults:

```toml
# This overrides nucleotide.toml timeout for Rust projects only
[[projects]]
name = "rust"
[projects.lsp]
startup_timeout_ms = 8000  # Rust needs more time
```

### Configuration Precedence

1. **Project-specific settings** in project_markers.toml (highest priority)
2. **Global LSP settings** in project_markers.toml
3. **LSP settings** in nucleotide.toml
4. **Built-in defaults** (lowest priority)

### Theme and UI Integration

Project markers can influence UI appearance:

```toml
[[projects]]
name = "rust"
[projects.ui]
# Optional: project-specific UI customizations
file_tree_icon = "🦀"
status_bar_color = "#f74c00"
project_badge = true
```

## Migration from File-Based to Project-Based LSP

### Step 1: Enable Project LSP Mode

Update your `nucleotide.toml`:

```toml
[lsp]
project_lsp_startup = true
enable_fallback = true  # Keep file-based as fallback during transition
```

### Step 2: Create Basic Project Markers

Start with a minimal configuration:

```toml
# Basic project_markers.toml
version = "1.0"

[global]
max_depth = 8
timeout_seconds = 5

# Add your most common project types first
[[projects]]
name = "rust"
markers = ["Cargo.toml"]
[projects.lsp]
servers = ["rust-analyzer"]

[[projects]]
name = "typescript"
markers = ["tsconfig.json", "package.json"]
required_patterns = ["tsconfig.json"]
[projects.lsp]
servers = ["typescript-language-server"]
```

### Step 3: Test and Refine

Monitor the logs for project detection:

```bash
# Check detection logs
tail -f ~/.local/share/nucleotide/nucleotide.log.$(date +%Y-%m-%d) | grep "project"
```

Common issues during migration:
- **Multiple matches**: Adjust priorities or add exclusion patterns
- **Missing detections**: Lower priority or add more markers
- **Slow detection**: Add exclusion patterns or reduce max_depth

### Step 4: Disable Fallback

Once confident in your configuration:

```toml
[lsp]
project_lsp_startup = true
enable_fallback = false  # Full project-based mode
```

## Troubleshooting Guide

### Common Configuration Issues

#### Issue: Projects Not Detected

**Symptoms**:
- LSP servers don't start automatically
- File tree shows no project indicators
- Status bar shows "No project detected"

**Solutions**:

1. **Check file patterns**:
   ```toml
   # Add more markers if needed
   markers = ["Cargo.toml", "Cargo.lock", "rust-project.json"]
   ```

2. **Verify max_depth**:
   ```toml
   [global]
   max_depth = 15  # Increase if project root is deep
   ```

3. **Check exclusion patterns**:
   ```toml
   # Remove overly broad exclusions
   exclude_patterns = []  # Temporarily disable
   ```

#### Issue: Wrong Project Root Detected

**Symptoms**:
- LSP starts in wrong directory
- Completions missing for project files
- Incorrect project name in status bar

**Solutions**:

1. **Adjust detection strategy**:
   ```toml
   [projects.root_detection]
   strategy = "workspace_aware"  # Try different strategies
   prefer_workspace = false
   ```

2. **Add workspace indicators**:
   ```toml
   workspace_indicators = [
       { pattern = "Cargo.toml", content_contains = ["[workspace]"] }
   ]
   ```

#### Issue: Multiple Project Types Detected

**Symptoms**:
- Conflicting LSP servers start
- Inconsistent project detection
- Performance issues from too many servers

**Solutions**:

1. **Adjust priorities**:
   ```toml
   [[projects]]
   name = "rust"
   priority = 160  # Higher priority

   [[projects]]
   name = "generic"
   priority = 90   # Lower priority
   ```

2. **Add exclusion patterns**:
   ```toml
   [[projects]]
   name = "javascript"
   exclude_patterns = ["Cargo.toml", "*.rs"]  # Don't match if Rust files present
   ```

#### Issue: LSP Servers Fail to Start

**Symptoms**:
- LSP timeouts in logs
- No diagnostics/completions
- Error messages in status bar

**Solutions**:

1. **Increase timeouts**:
   ```toml
   [projects.lsp]
   startup_timeout_ms = 15000  # Java/C++ need more time
   ```

2. **Check server configuration**:
   ```toml
   [projects.lsp.server_config.rust-analyzer]
   command = "/usr/local/bin/rust-analyzer"  # Full path if needed
   args = []
   env = { "RUST_LOG" = "info" }  # Add debugging
   ```

3. **Enable health checks**:
   ```toml
   [projects.lsp.health_check]
   enabled = true
   interval_seconds = 30
   failure_threshold = 2
   ```

### Performance Issues

#### Issue: Slow Project Detection

**Symptoms**:
- Long delays when opening files
- High CPU usage during detection
- Timeout errors

**Solutions**:

1. **Add exclusion patterns**:
   ```toml
   [projects.file_patterns]
   ignore = [
       "node_modules/**",
       "target/**",
       ".git/**",
       "**/build/**"
   ]
   ```

2. **Reduce search depth**:
   ```toml
   [global]
   max_depth = 6  # Reduce from default 10
   ```

3. **Disable parallel detection**:
   ```toml
   [global]
   enable_parallel_detection = false  # If causing issues
   ```

#### Issue: Too Many LSP Servers

**Symptoms**:
- High memory usage
- System slowdown
- LSP conflicts

**Solutions**:

1. **Limit concurrent servers**:
   ```toml
   [global.lsp]
   max_concurrent_servers = 3  # Reduce from default 5
   ```

2. **Be selective with servers**:
   ```toml
   [projects.lsp]
   servers = ["rust-analyzer"]  # Only essential servers
   ```

### Validation and Testing

#### Configuration Validation

Use the built-in validator:

```bash
# Validate your configuration
nucleotide config validate project-markers

# Check for common issues
nucleotide config lint project-markers

# Test project detection
nucleotide config test-detection /path/to/project
```

#### Debug Mode

Enable debug logging:

```toml
# In nucleotide.toml
[debug]
project_detection = true
lsp_startup = true
```

Or via environment variable:
```bash
NUCLEOTIDE_DEBUG=project_detection,lsp_startup nucleotide
```

#### Test Configuration

Create a test configuration:

```toml
# test_project_markers.toml
version = "1.0"

[global]
max_depth = 5  # Faster for testing

[[projects]]
name = "test-project"
priority = 200
markers = [".test-project"]  # Use a test marker

[projects.lsp]
servers = []  # No actual LSP servers for testing
```

Test with:
```bash
# Create test marker
touch .test-project

# Test detection
nucleotide --config-file test_project_markers.toml config test-detection .
```

### Advanced Configuration Examples

#### Multi-Language Monorepo

```toml
[[projects]]
name = "monorepo-root"
priority = 200
markers = [".monorepo-root", "workspace.toml"]

[projects.root_detection]
strategy = "outermost"
prefer_workspace = true

# Multiple language support
[projects.lsp]
servers = ["rust-analyzer", "typescript-language-server", "pyright"]

# Sub-project detection
[[projects]]
name = "rust-service"
priority = 150
markers = ["Cargo.toml"]
required_patterns = ["src/main.rs", "src/lib.rs"]
parent_patterns = [".monorepo-root"]  # Must be in monorepo

[[projects]]
name = "web-frontend"  
priority = 150
markers = ["package.json", "tsconfig.json"]
parent_patterns = [".monorepo-root"]
```

#### Language-Specific Workspaces

```toml
# Rust workspace with intelligent detection
[[projects]]
name = "rust-workspace"
priority = 160
markers = ["Cargo.toml"]

[projects.root_detection] 
strategy = "workspace_aware"
workspace_indicators = [
    { pattern = "Cargo.toml", content_contains = ["[workspace]"] }
]

# Individual workspace members
[[projects]]
name = "rust-member"
priority = 140  
markers = ["Cargo.toml"]
exclude_patterns = ["[workspace]"]  # Don't match workspace roots

[projects.root_detection]
strategy = "innermost"  # Find the specific member
```

#### Conditional LSP Servers

```toml
[[projects]]
name = "python-data-science"
markers = ["requirements.txt", "environment.yml"]
required_patterns = ["*.ipynb", "*.py"]

[projects.lsp]
# Start different servers based on project contents
servers = ["pyright"]

# Conditional server configuration
[projects.lsp.conditional]
[[projects.lsp.conditional.servers]]
condition = { file_exists = "requirements.txt", content_contains = "jupyter" }
servers = ["pyright", "jupyter-lsp"]

[[projects.lsp.conditional.servers]]  
condition = { file_exists = "environment.yml", content_contains = "tensorflow" }
servers = ["pyright", "tensorflow-lsp"]
```

This comprehensive configuration system provides the flexibility and power needed for modern development workflows while maintaining performance and reliability.