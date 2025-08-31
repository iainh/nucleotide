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
