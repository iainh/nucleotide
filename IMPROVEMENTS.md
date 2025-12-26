# Nucleotide Improvement Plan

> Consolidated recommendations from Oracle and Librarian analysis, cross-reviewed for accuracy and prioritization.

## Executive Summary

This document outlines actionable improvements to the Nucleotide codebase, organized by priority tier. Focus areas include: dependency hygiene, architecture cleanup, testability, and developer experience.

---

## Priority Tiers

### 🔴 TIER 0 — Critical (Immediate Action Required)

#### 1. Audit Tokio Runtime Integration with GPUI
**Impact:** ⭐⭐⭐⭐⭐ | **Effort:** M | **Risk if ignored:** Deadlocks, performance issues

**Problem:** Potential dual-runtime architecture where GPUI and Helix/LSP spawn separate tokio runtimes.

**Diagnostic:**
```bash
grep -r "tokio::spawn\|Runtime::new\|#\[tokio::" crates/*/src/*.rs
grep -r "spawn_blocking\|block_on" crates/*/src/*.rs
```

**Action:**
- Verify all `tokio::spawn()` calls use GPUI's integrated runtime
- Ensure zero `tokio::runtime::Runtime::new()` calls outside main initialization
- Document the runtime architecture in ARCHITECTURE.md

---

#### 2. Config Validation Must Enforce Errors
**Impact:** ⭐⭐⭐⭐⭐ | **Effort:** S | **Location:** `crates/nucleotide-lsp/src/coordination_manager.rs`

**Problem:** Validation warns but returns `Ok(())` — users get silent failures.

**Fix:**
```rust
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Project LSP startup enabled without fallback - this may cause startup failures")]
    FallbackRequiredWithProjectLsp,
}

pub fn validate(&self) -> Result<(), ConfigError> {
    if self.project_lsp_startup && !self.enable_fallback {
        return Err(ConfigError::FallbackRequiredWithProjectLsp);
    }
    Ok(())
}
```

**Additional:** Add `--check-config` CLI flag for debugging.

---

#### 3. Pin Helix Fork to Specific Commit
**Impact:** ⭐⭐⭐⭐⭐ | **Effort:** S | **Location:** Root `Cargo.toml`

**Problem:** Helix deps pinned to floating branch `nucleotide-lsp-integration` — builds are non-reproducible.

**Fix:**
```toml
# Change from:
helix-core = { git = "https://github.com/iainh/helix", branch = "nucleotide-lsp-integration" }

# To:
helix-core = { git = "https://github.com/iainh/helix", rev = "abc1234def5678" }
```

**Document:** Add HELIX_UPDATE.md explaining how to update the fork.

---

### 🟠 TIER 1 — High Priority (This Sprint)

#### 4. Unify Dependency Versions via Workspace
**Impact:** ⭐⭐⭐⭐ | **Effort:** S | **Time:** 30 min

**Problem:** Version mismatches across crates:
- `nucleotide-events`: `thiserror = "2.0"` (workspace: `1.0`)
- `nucleotide-logging`: `dirs = "6.0"` (workspace: `5.0`)
- `nucleotide-project`: `tempfile = "3.0"` (workspace: `3.10`)
- `nucleotide-editor`: `rand = "0.9"` (workspace: `0.8`)

**Fix:** Convert all inline versions to workspace references:
```toml
# Before:
tokio = { version = "1.32", features = ["sync"] }

# After:
tokio = { workspace = true, features = ["sync"] }
```

**Crates to update:**
- [ ] `nucleotide-events/Cargo.toml`
- [ ] `nucleotide-logging/Cargo.toml`
- [ ] `nucleotide-project/Cargo.toml`
- [ ] `nucleotide-editor/Cargo.toml`
- [ ] `nucleotide/Cargo.toml` (chrono)

---

#### 5. Replace Mutex with parking_lot::Mutex in Event Aggregator
**Impact:** ⭐⭐⭐⭐ | **Effort:** S | **Location:** `crates/nucleotide-core/src/event_aggregator.rs`

**Problem:** Uses `std::sync::Mutex` with `.lock().unwrap()` — panics on lock poisoning.

**Fix:**
```rust
// Before:
use std::sync::Mutex;
let mut handlers = self.handlers.lock().unwrap();

// After:
use parking_lot::Mutex;
let mut handlers = self.handlers.lock();  // Non-poisoning, faster
```

**Note:** `parking_lot` is already in dependencies. Profile-guided optimization if needed.

---

#### 6. Add Safety Comments to Unsafe Blocks
**Impact:** ⭐⭐⭐ | **Effort:** S | **Location:** `crates/nucleotide/src/main.rs`

**Context:** The `unsafe { std::env::set_var(...) }` calls are *justified* in `#[ctor]` context (before threading) but lack documentation.

**Fix:** Add SAFETY comments:
```rust
// SAFETY: This is called from #[ctor] before any threads are spawned.
// std::env::set_var is only unsafe when called concurrently with other
// env operations. In #[ctor] context, we're guaranteed single-threaded execution.
unsafe { std::env::set_var("HELIX_RUNTIME", &rt) };
```

---

#### 7. Centralize Logging (Verify Complete)
**Impact:** ⭐⭐⭐ | **Effort:** S | **Status:** Mostly done

**Audit:** Ensure no crates import `tracing-subscriber` directly (except `nucleotide-logging`):
```bash
grep -r "use tracing_subscriber" crates/*/src/*.rs | grep -v nucleotide-logging
```

**Fix any violations:** Replace with `nucleotide_logging::*` re-exports.

---

### 🟡 TIER 2 — Medium Priority (Next Sprint)

#### 8. Break Cyclic Dependency: project → lsp → ui
**Impact:** ⭐⭐⭐⭐⭐ | **Effort:** L | **Risk:** Large refactor

**Problem:** `nucleotide-project` depends on `nucleotide-ui` for rendering; `nucleotide-lsp` also depends on UI components. This creates tight coupling.

**Incremental Plan:**
1. Identify shared concepts causing the cycle (types, traits, services)
2. Move to `nucleotide-types` or new `nucleotide-interfaces` crate
3. Ensure one-way dependency flow: `types ← project ← lsp ← ui`
4. Use trait objects for UI integration instead of direct imports

**Prerequisite:** Add tests for affected modules before refactoring.

---

#### 9. Refactor Global Event Bridge (OnceLock)
**Impact:** ⭐⭐⭐⭐ | **Effort:** M | **Location:** `crates/nucleotide-core/src/event_bridge.rs`

**Problem:** `static EVENT_BRIDGE_SENDER: OnceLock<...>` is hard to test and reset.

**Plan:**
1. Define `EventBridge` trait with minimal methods
2. Wrap global behind trait-object accessor
3. Provide test injection via `set_event_bridge_for_test`
4. Migrate callers from global access to dependency injection

---

#### 10. Unify Workspace Root Detection
**Impact:** ⭐⭐⭐⭐ | **Effort:** M | **Locations:**
- `crates/nucleotide/src/application/mod.rs`
- `crates/nucleotide-lsp/src/coordination_manager.rs`

**Problem:** Two implementations with different marker sets:
```rust
// application/mod.rs:
const VCS_DIRS: &[&str] = &[".git", ".helix", ".hg", ".jj", ".svn"];

// coordination_manager.rs:
if ancestor.join(".git").exists() || ancestor.join(".svn").exists() { ... }
```

**Fix:**
1. Create `nucleotide_types::project::find_workspace_root()`
2. Add tests for edge cases (nested repos, symlinks, missing markers)
3. Replace all duplicate implementations

---

#### 11. Add Workspace-Level Linting Defaults
**Impact:** ⭐⭐⭐ | **Effort:** S | **Time:** 10 min

**Add to root `Cargo.toml`:**
```toml
[workspace.lints.rust]
unsafe_code = "warn"
unused_imports = "warn"

[workspace.lints.clippy]
all = "warn"
pedantic = "allow"
nursery = "allow"
```

**Then in each crate:**
```toml
[lints]
workspace = true
```

---

#### 12. Create justfile for Common Commands
**Impact:** ⭐⭐⭐ | **Effort:** S | **Time:** 20 min

**Create `justfile`:**
```just
# Build
build:
    cargo build --workspace

build-release:
    cargo build --release

# Testing
test:
    cargo test --workspace

# Linting
lint:
    cargo clippy --workspace --all-targets

format:
    cargo fmt --all

format-check:
    cargo fmt --all -- --check

# Combined
check: lint format-check test

# Development
dev:
    cargo run -p nucleotide

# Dependencies
deps-check:
    cargo deny check
    cargo machete

# Architecture
arch-check:
    ./scripts/check-layering.sh
```

---

#### 13. Run cargo-machete/udeps to Trim Dead Deps
**Impact:** ⭐⭐⭐ | **Effort:** S

**Commands:**
```bash
cargo install cargo-machete
cargo machete --all-features

cargo +nightly install cargo-udeps
cargo +nightly udeps --all-targets --workspace
```

**Suspected unused:** `chardetng`, `ignore`, `regex` (verify before removing).

**Add to CI:** Pre-commit check or CI step.

---

### 🟢 TIER 3 — Lower Priority (Backlog)

#### 14. Add Unit Tests for Core Modules
**Impact:** ⭐⭐⭐⭐⭐ | **Effort:** XL (ongoing)

**Missing coverage:**
- `nucleotide-core`: No visible unit tests
- `nucleotide-types`: No tests
- `nucleotide-events`: No tests

**Strategy:**
1. Identify critical invariants (editing, serialization, event ordering)
2. Add tests before any refactor of a module
3. Focus on round-trip and regression tests

**Immediate targets:** Modules touched by Tier 0-2 changes.

---

#### 15. Investigate Broadcast Channel Capacity
**Impact:** ⭐⭐⭐ | **Effort:** M | **Location:** `crates/nucleotide-lsp/src/project_lsp_manager.rs`

**Problem:** `broadcast::channel(1000)` can drop events if subscribers lag.

**Action:**
1. Add logging for send failures / recv lag errors
2. Document which events are lossy-tolerant vs loss-intolerant
3. Consider `mpsc` with backpressure for critical events

---

#### 16. Make nucleotide-events Lighter (Optional)
**Impact:** ⭐⭐⭐ | **Effort:** M

**Current:** nucleotide-events depends on `tokio`, `async-trait`, and Helix crates.

**If pursued:**
1. Keep lightweight event types in nucleotide-events (pure enums/structs)
2. Move `EventHandler<E>` + async machinery to `nucleotide-event-handlers` (Layer 3)
3. nucleotide-core imports event-handlers for async implementations

**Verdict:** Low priority unless it becomes a compilation bottleneck.

---

#### 17. Implement Syntax Highlighting (Feature)
**Impact:** ⭐⭐⭐⭐⭐ (UX) | **Effort:** L | **Location:** `crates/nucleotide-editor/src/document_renderer.rs`

**Current:** `render_with_highlighting()` is stubbed to call `render_document()`.

**Plan:**
1. Start with key languages: Rust, TOML, Markdown
2. Reuse Helix's theme & grammar definitions
3. Add benchmarks for large-file performance
4. Track via GitHub issue

---

#### 18. Coalesce Periodic Timers (Profile First)
**Impact:** ⭐⭐⭐ | **Effort:** M

**Reported timers:** Spinner (80ms), step loop (200ms), maintenance (200ms).

**Action:**
1. Find all `Duration::from_millis`, `interval`, `tick` calls
2. Profile CPU impact
3. If significant, merge into single ~100ms tick

---

## Documentation to Create

### ARCHITECTURE.md
- High-level crate dependency graph
- Layer definitions (0-6 as per AGENTS.md)
- Major flows: startup, event routing, workspace detection
- Where to add new features

### TROUBLESHOOTING.md
- Config validation failures
- LSP connection issues
- Workspace detection problems
- Common runtime panics and log collection

---

## Summary Checklist

| Priority | Item | Effort | Status |
|----------|------|--------|--------|
| 🔴 P0 | Audit Tokio runtime integration | M | ⬜ |
| 🔴 P0 | Config validation must enforce errors | S | ⬜ |
| 🔴 P0 | Pin Helix fork to commit hash | S | ⬜ |
| 🟠 P1 | Unify dependency versions | S | ⬜ |
| 🟠 P1 | Replace Mutex with parking_lot | S | ⬜ |
| 🟠 P1 | Add safety comments to unsafe blocks | S | ⬜ |
| 🟠 P1 | Verify centralized logging | S | ⬜ |
| 🟡 P2 | Break cyclic dependency | L | ⬜ |
| 🟡 P2 | Refactor global event bridge | M | ⬜ |
| 🟡 P2 | Unify workspace root detection | M | ⬜ |
| 🟡 P2 | Add workspace linting defaults | S | ⬜ |
| 🟡 P2 | Create justfile | S | ⬜ |
| 🟡 P2 | Run cargo-machete/udeps | S | ⬜ |
| 🟢 P3 | Add unit tests for core modules | XL | ⬜ |
| 🟢 P3 | Investigate broadcast channel | M | ⬜ |
| 🟢 P3 | Make nucleotide-events lighter | M | ⬜ |
| 🟢 P3 | Implement syntax highlighting | L | ⬜ |
| 🟢 P3 | Coalesce periodic timers | M | ⬜ |

---

## Cross-Review Notes

### Oracle's Refinements to Librarian's Suggestions:
- **Lock contention (#1):** Only use `parking_lot` in hot paths; add instrumentation first
- **Error refactor (#4):** Don't purge `anyhow` everywhere — focus on public APIs and user-facing failures
- **Broadcast channel (#10):** Not P0 unless observing actual event loss; start with measurement
- **Tests (#6):** Scope to critical invariants, not blanket coverage

### Librarian's Refinements to Oracle's Suggestions:
- **Unsafe blocks (#5):** These are *justified* in `#[ctor]` context — don't remove, just document
- **HELIX_RUNTIME duplication (#6):** May not exist in current code — audit first
- **#[ctor] weight (#7):** Profile before optimizing; may be acceptable trade-off
- **Helix-agnostic events (#10):** Low value unless testing requires mocking Helix types

---

*Generated: 2024-12-26 | Cross-reviewed by Oracle and Librarian*
