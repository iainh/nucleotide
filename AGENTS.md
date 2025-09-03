# Repository Guidelines

## Project Structure & Module Organization
- Root is a Rust workspace (`Cargo.toml`) with crates under `crates/`.
- App binary: `crates/nucleotide` (bin name `nucl`). Shared libraries live in sibling crates (e.g., `nucleotide-core`, `-editor`, `-ui`, `-lsp`, `-events`, `-types`).
- Docs in `docs/`; CI in `.github/workflows/`; helper scripts in `scripts/`.
- Tests live alongside code (e.g., `src/tests/*.rs`, `*_tests.rs`) and in each crate.

## Build, Test, and Development Commands
- Build all: `cargo build --workspace` (release: `cargo build --release`).
- Run app: `cargo run -p nucleotide` (produces `nucl`).
- Test all crates: `cargo test --all` (CI runs this).
- Format check: `cargo fmt --all -- --check` (install hooks: `./scripts/install-hooks.sh`).
- Architecture check: `./scripts/check-layering.sh`.
- Dependency checks: `cargo deny check` and `cargo +nightly udeps --all-targets --workspace`.
- macOS bundle: `./bundle-mac.sh` then `open Nucleotide.app`.

## Coding Style & Naming Conventions
- Rust 2024 edition; format with `rustfmt` (2‑space or default rustfmt indentation).
- Names: crates/kinds use kebab-case; modules/functions snake_case; types/enums PascalCase; constants SCREAMING_SNAKE_CASE.
- Prefer small, layered dependencies (see `scripts/check-layering.sh`). Keep `nucleotide-types` light without extra features.

## Testing Guidelines
- Use standard Rust tests with `#[test]`; integration tests live under `src/tests` or dedicated files like `*_tests.rs`.
- Run locally with `cargo test --all`; filter with `cargo test <name>`.
- Add focused tests near the code you change; avoid cross‑crate coupling in tests.

## Commit & Pull Request Guidelines
- Use Conventional-style prefixes: `feat:`, `fix:`, `perf:`, `chore:`, `style:`, `remove:`, `migrate:`, `enhance:`, `config:`.
- PRs must: describe the change, link issues, include screenshots for UI changes, and pass CI (fmt, tests, layering, dependency checks).
- Keep commits atomic and scoped to a single concern; update docs in `docs/` when behavior changes.

## Security & Configuration Tips
- App config: `~/.config/helix/nucleotide.toml` (falls back to Helix `config.toml`).
- Useful env vars: `RUST_LOG=info` for logs; `HELIX_RUNTIME` when running from a bundle (set automatically on macOS).

## Event System
- Overview: Bidirectional GPUI ↔ Helix system with domain events and bridges. See `docs/event_system.md` for diagrams and flows.
- Crate: `crates/nucleotide-events` defines domain events (`v2::{document, view, editor, lsp, ui, workspace, vcs}`) and the bus (`event_bus.rs`).
- Emit: Use `EventBus::dispatch_{document,editor,ui,workspace,lsp,vcs}(... )` for publishing events; avoid ad‑hoc channels.
- Handle: Implement `EventHandler` and override `handle_*` methods scoped to your domain.
- Bridges: GPUI→Helix translation and Helix hooks live under `crates/nucleotide/src/application/*` and `crates/nucleotide-events/src/{bridge,ui,editor,document,...}.rs`.
- Best practices:
  - Prefer domain events over integration/legacy events; keep handlers fast and side‑effect‑free when possible.
  - Don’t block the app loop; offload heavy work to async tasks and send results back as events.
  - Put focused tests near handlers and new event types.

## Design Tokens
- Module: `crates/nucleotide-ui/src/tokens` with `DesignTokens`, `SemanticColors`, `SizeTokens`. Read `crates/nucleotide-ui/src/tokens/README.md`.
- Layers: Base colors → semantic colors → component tokens. Consistent spacing scale and elevation helpers.
- Use:
  - Access via `Theme.tokens` (e.g., `theme.tokens.colors.text_primary`, `theme.tokens.sizes.space_3`).
  - Prefer semantic tokens over hardcoded colors or px values; use utilities like `lighten`, `darken`, `with_alpha`.
  - Components map tokens to styles (see `crates/nucleotide-ui/src/button.rs` compute style from tokens).
- Migration: Legacy theme fields remain (e.g., `Theme::dark()`); new work should favor tokens and `Theme::from_tokens(...)`.

## UI Paradigms
- Providers: `crates/nucleotide-ui/src/providers` offers React‑style providers for theme, config, and events.
  - Compose: `ProviderComposition::app_providers()` or build via `provider_tree()`.
  - Access: `use_theme()`, `use_provider<T>()`, `use_provider_or_default<T>()`.
- Styling: Use `Styled`, `ComponentFactory`, and `Variant*` types from `nucleotide-ui::styling` to compute styles from tokens/state.
- Keyboard & Focus: Centralized input handling in `nucleotide-ui::{global_input, keyboard_navigation}` with focus rings and navigation helpers.
- Popups & Layout: Use `completion_popup` and sizing utilities for anchored overlays; avoid manual positioning where helpers exist.
- Theming: `theme_manager`, `advanced_theming` support runtime switching and Helix theme bridge while keeping token‑first APIs.

## Where To Add Things
- New domain events: `crates/nucleotide-events/src/v2/<domain>/` plus handler wiring in `crates/nucleotide/src/application/*`.
- New component tokens: extend `crates/nucleotide-ui/src/tokens/mod.rs` and add tests in `tokens/tests.rs`.
- New UI components: `crates/nucleotide-ui/src/` with local tests (e.g., `*_tests.rs`).
- Logging/metrics: use `nucleotide-logging` macros and layers (`crates/nucleotide-logging`).

## References
- Event system: `docs/event_system.md`
- Token system: `crates/nucleotide-ui/src/tokens/README.md`
- UI theming & providers: `crates/nucleotide-ui/src/{providers,theme_manager,advanced_theming}`

## Version Control (jj + Git)
- Purpose: jj (Jujutsu) provides easy stacked changes and rebases while keeping Git as the remote/CI source of truth.
- New clones: `jj git clone <git-url> nucleotide && cd nucleotide` (creates a jj workspace backed by Git).
- Existing Git clone: run `jj git import` once in the repo to initialize jj state.
- Status & review:
  - `jj status` (working copy + current change)
  - `jj log -r ::@` (your stack) and `jj diff -s` (summary of current change)
- Create/shape changes:
  - Start a change: `jj new -m "feat: short message"` then edit files.
  - Update message: `jj describe -m "refine: clearer message"`.
  - Split or squash when cleaning up: `jj split` and `jj squash`.
  - Rebase your stack on latest main: `jj git fetch && jj git import && jj rebase -d main -r ::@`.
- Branches and publishing:
  - Point a Git branch at the current change: `jj branch set my-branch -r @`.
  - Push to remote: `jj git push -b my-branch` (updates the Git branch; CI/PRs see normal Git commits).
  - Pull remote updates: `jj git fetch && jj git import`.
- Conventions:
  - Keep Conventional Commit prefixes (feat:, fix:, chore:, etc.) in `jj` change messages.
  - Prefer small, reviewable stacks over large single changes; rebase/squash before pushing.
  - Teammates can continue to use plain Git; jj changes appear as normal Git commits/branches.

### jj Config (recommended)
- Scope: add to user config `~/.jjconfig` (applies everywhere) or set per-repo via `jj config set --repo ...`.
- Minimal TOML snippet:
  ```toml
  [ui]
  # Show a compact graph by default in `jj log`
  default-command = "log"

  [aliases]
  # My stack (ancestors and descendants of the current change)
  l = ["log", "-r", "::@", "-T", "builtin_log_detailed"]
  # Quick summary of current change
  d = ["diff", "-s"]
  # Rebase current stack onto main
  rebase-stack = ["rebase", "-d", "main", "-r", "::@"]
  # Show heads for your stack (useful before pushing)
  heads = ["log", "-r", "heads(::@) - hidden()", "-T", "builtin_log_compact"]
  ```
- Or set via commands (repo-scoped):
  - `jj config set --repo aliases.l '["log", "-r", "::@", "-T", "builtin_log_detailed"]'`
  - `jj config set --repo aliases.d '["diff", "-s"]'`
  - `jj config set --repo aliases.rebase-stack '["rebase", "-d", "main", "-r", "::@"]'`
