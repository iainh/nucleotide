────────────────────────────────────────────────────────────────────────
 IMPLEMENTATION PLAN – NUCLEOTIDE LAYERED-ARCHITECTURE REFACTOR (PHASE 2.5)
────────────────────────────────────────────────────────────────────────
Legend
• $ denotes shell command typed from repository root
• ⇒ means "expect this result"
• <…> placeholders must be replaced literally in code
• All file paths are absolute from workspace root unless prefixed with crates/<crate-name>/

Target finish criteria
✓ Root workspace crate is crates/nucleotide-workspace; legacy crates/nucleotide only glues UI & editor.
✓ crates/nucleotide-editor no longer depends on crates/nucleotide-ui (unidirectional dependency restored).
✓ crates/nucleotide-types compiles with zero gpui / ratatui / crossterm / helix-* dependencies.
✓ Every Cargo.toml contains standardised metadata derived from [workspace.package].
✓ LAYERED_ARCHITECTURE.md and crate-level READMEs reflect final state.
✓ CI job fails if a crate violates layering or introduces disallowed external deps.

High-level steps
1. Preparation & branch management
2. Complete workspace migration
3. Break editor → ui dependency
4. Purge GUI deps from nucleotide-types
5. Harmonise Cargo metadata & features
6. Documentation sweep
7. CI dependency validation
8. Verification matrix
9. Rollback instructions

────────────────────────────────────────────────────────────────────────
1. Preparation & branch management
────────────────────────────────────────────────────────────────────────
1.1 $ git checkout main && git pull  
1.2 $ git checkout -b refactor/phase-2-5  
1.3 Add a signed tag so we can roll back:
    $ git tag -s pre-phase-2-5 -m "Checkpoint before final layered refactor"

────────────────────────────────────────────────────────────────────────
2. Complete workspace migration: crates/nucleotide-workspace
────────────────────────────────────────────────────────────────────────
HEAD status
• crates/nucleotide still contains application entry + workspace code.
• crates/nucleotide-workspace is a shell.

Goal
Move "workspace / layout / tab" logic into crates/nucleotide-workspace and make crates/nucleotide depend on it, keeping only main.rs / config loading there.

2.1 Create module skeleton in nucleotide-workspace

crates/nucleotide-workspace/src/
    lib.rs           // re-exports public API
    manager.rs       // WorkspaceManager { … }
    layout.rs        // LayoutState, Panel, …
    tabs.rs          // Tab, TabGroup, …

2.2 Grep for code to migrate
$ rg --glob "crates/nucleotide/**" "(TabManager|WorkspaceManager|layout_|panel_|tab_)" 

2.3 Move files
For every hit inside crates/nucleotide/src/ that matches workspace concerns:

    mv crates/nucleotide/src/<file>.rs crates/nucleotide-workspace/src/<same>.rs
    // adjust mod declarations afterwards.

2.4 Replace internal module paths
Use `cargo fix --workspace --allow-dirty` after the move to auto-update imports.
Manual touch-up example:
    // BEFORE
    use crate::layout::LayoutState;
    // AFTER (inside editor/ui crates)
    use nucleotide_workspace::layout::LayoutState;

2.5 Add public re-exports in crates/nucleotide-workspace/src/lib.rs:

pub mod manager;
pub mod layout;
pub mod tabs;

pub use manager::{WorkspaceManager, WorkspaceId};
pub use layout::{LayoutState, Panel, PanelId};
pub use tabs::{Tab, TabGroup};

2.6 Add crate dep
Open crates/nucleotide/Cargo.toml:

[dependencies]
nucleotide-workspace = { path = "../nucleotide-workspace" }
# REMOVE any workspace-level code modules from lib.rs

2.7 Trim crates/nucleotide
• Delete src/workspace/, src/layout/, src/tab* rs.
• Ensure main.rs now does:

fn main() {
    env_logger::init();
    nucleotide::app::run();
}

and crates/nucleotide/src/lib.rs (if exists) ONLY glues gpui::App + workspace.

2.8 Remove duplicate types
If WorkspaceId, TabId etc already exist in nucleotide-types move them into types crate; otherwise create them there and update references.  (see step 4).

2.9 Compile
$ cargo check -p nucleotide-workspace  
⇒ succeeds

────────────────────────────────────────────────────────────────────────
3. Break nucleotide-editor → nucleotide-ui dependency
────────────────────────────────────────────────────────────────────────

Currently: crates/nucleotide-editor/Cargo.toml
[dependencies]
nucleotide-ui = { path = "../nucleotide-ui" }  // must be removed

Plan: introduce capability traits in nucleotide-core, implemented in ui.

3.1 Identify usages
$ rg --glob "crates/nucleotide-editor/**/*.{rs}" "nucleotide_ui::" -n

Expect calls like `nucleotide_ui::theme::CurrentTheme::…`, `ui::completion::show_completion`.

Log each distinct call in a TODO list.

3.2 Design traits (in crates/nucleotide-core/src/capabilities/)

Create new file crates/nucleotide-core/src/capabilities/ui_bridge.rs:

pub trait ThemeProvider {
    fn current_theme() -> Theme;
}
pub trait CompletionUI {
    fn show_completion(items: &[CompletionItem]);
    fn hide_completion();
}

Export them in crates/nucleotide-core/src/lib.rs:

pub mod capabilities;

3.3 Remove direct calls in editor
Edit each use site (use sed or manual):

// BEFORE
use nucleotide_ui::completion::show_completion;
show_completion(&items);

// AFTER
use nucleotide_core::capabilities::CompletionUI;
<U as CompletionUI>::show_completion(&items);
// Where U is a generic ctx param or use a global impl via 'nucleotide_ui_bridge' (next step)

Simplest path: create a façade module in editor:

pub struct DefaultUIBridge;
impl CompletionUI for DefaultUIBridge {
    fn show_completion(items: &[CompletionItem]) {
        nucleotide_ui::completion::show_completion(items);
    }
    …
}

Editor code then depends only on trait.

BUT to avoid ui dep we move DefaultUIBridge into nucleotide-ui-bridge crate inside ui (layer 5). So:

3.3.1 Create new sub-crate crates/nucleotide-ui-bridge (optional) OR add module inside nucleotide-ui:

crates/nucleotide-ui/src/bridge.rs:

use nucleotide_core::capabilities::{CompletionUI, ThemeProvider};
pub struct UiBridge;
impl CompletionUI for UiBridge {
    fn show_completion(items: &[CompletionItem]) {
        crate::completion::show_completion(items)
    }
    fn hide_completion() { crate::completion::hide_completion() }
}
impl ThemeProvider for UiBridge { … }

Export with `pub use bridge::UiBridge;`

3.4 Remove dependency
crates/nucleotide-editor/Cargo.toml: delete nucleotide-ui line. Add:

[dependencies]
nucleotide-core = { path = "../nucleotide-core" }

3.5 Inject bridge
In crates/nucleotide/src/main.rs right after gpui init:

use nucleotide_ui::UiBridge;
nucleotide_editor::set_ui_bridge::<UiBridge>();

Implement in editor:

static UI_BRIDGE: once_cell::sync::OnceCell<&'static dyn CompletionUI> = …;
pub fn set_ui_bridge<B: CompletionUI + 'static>() { UI_BRIDGE.set(&B).unwrap() }

Editor internals call UI_BRIDGE.get().unwrap().show_completion(…).

3.6 Compile & test

$ cargo check -p nucleotide-editor
$ cargo test -p nucleotide-editor

3.7 Delete leftover import lines

rg -l "nucleotide_ui::" crates/nucleotide-editor | xargs sed -i '' '/nucleotide_ui::/d'

────────────────────────────────────────────────────────────────────────
4. Remove heavy GUI dependencies from nucleotide-types
────────────────────────────────────────────────────────────────────────
Goal: Only serde, std, optional no_std crates allowed.

4.1 Inspect Cargo.toml
open crates/nucleotide-types/Cargo.toml  
Remove lines:
gpui = …  
ratatui / crossterm / helix-*

4.2 Identify offending use
$ rg --glob "crates/nucleotide-types/**/*" "(gpui|Color|Px)" 

When types depend on gpui types like gpui::Color convert them to own lightweight types.

Example fix (crates/nucleotide-types/src/color.rs):

// BEFORE
use gpui::Color;
#[derive(Clone, Serialize, Deserialize)]
pub struct ThemeColor(pub Color);

// AFTER
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

Provide From/Into impl only behind optional feature `gpui-bridge`.

4.3 Add feature gate in Cargo.toml:

[features]
default = []
gpui-bridge = ["dep:gpui"]

[dependencies]
serde = { … }
gpui = { workspace = true, optional = true }

4.4 Move FontSettings out
If FontSettings previously stored gpui::FontStretch etc: create thin structs with numeric fields.

4.5 Run lints

$ cargo check -p nucleotide-types --no-default-features
⇒ compiles with zero heavy deps

Then:

$ cargo check -p nucleotide-ui --features "nucleotide-types/gpui-bridge"
⇒ ui compiles.

────────────────────────────────────────────────────────────────────────
5. Standardise Cargo metadata
────────────────────────────────────────────────────────────────────────
5.1 Adopt workspace-wide metadata already in root Cargo.toml.

For each crate under crates/*:
• Delete redundant authors, license, version, edition if identical.
• Ensure name, description and categories present.

Example patch for crates/nucleotide-editor/Cargo.toml:
--- a/crates/nucleotide-editor/Cargo.toml
-[package]
-name = "nucleotide-editor"
-version = "0.1.0"
-authors = ["…"]
-edition = "2021"
+[package]
+name = "nucleotide-editor"
+publish = false
+description = "Text rendering & editing logic layer (Layer 4)."
+categories = ["text-editors","gui"]
+rust-version = "1.88"

Apply analogous changes for all.

5.2 Ensure each has:

[package.metadata.docs.rs]
all-features = true

────────────────────────────────────────────────────────────────────────
6. Documentation update
────────────────────────────────────────────────────────────────────────
6.1 Update LAYERED_ARCHITECTURE.md lines 95-110 status → all ✅ and remove TODO sections.

6.2 For each crate root create/refresh README.md with:
• Purpose
• Public API surface
• Dependency graph snippet (cargo tree –edges no-dev).

6.3 Run mdbook test if mdBook used.

────────────────────────────────────────────────────────────────────────
7. CI dependency validation
────────────────────────────────────────────────────────────────────────
Tooling: cargo-deny + cargo-udeps for dead-deps.

7.1 Add cargo-deny config .cargo-deny.toml at repo root:

[deny]
sources = "allow"
duplicate-crates = "deny"

[bans]
wildcards = "deny"

[advisories]
db-path = "~/.cargo/advisory-db"
ignore = []

[graph]
# Custom layering rules
deny-build-script-overrides = true
# forbid high->low deps
skip-tree = []

[targets]
# Restrict each layer to allowed external crates
[[targets.crate]]
name = "nucleotide-types"
allow = ["serde", "serde_json"]

… (repeat list)

7.2 GitHub Actions job .github/workflows/ci.yml:

- run: cargo deny check
- run: cargo udeps --all-targets --workspace

7.3 Add upsweep script scripts/check-layering.sh

#!/usr/bin/env bash
set -euo pipefail
cargo metadata --format-version 1 | jq '..|.name? // empty' > /tmp/crates.txt
# fail if "nucleotide-editor -> nucleotide-ui" appears
cargo tree -e normal -i nucleotide-editor | grep "nucleotide-ui" && exit 1 || true

Add to CI.

────────────────────────────────────────────────────────────────────────
8. Verification matrix
────────────────────────────────────────────────────────────────────────
After all steps:

8.1 $ cargo test --all  
8.2 $ cargo clippy --all-targets -- -D warnings  
8.3 $ cargo deny check  
8.4 Manual runtime test: `cargo run -p nucleotide` open files, switch tabs, completion pops.

8.5 Ensure `cargo publish --dry-run -p nucleotide-types` passes.

────────────────────────────────────────────────────────────────────────
9. Rollback procedure
────────────────────────────────────────────────────────────────────────
If any stage fails catastrophically:

$ git reset --hard pre-phase-2-5  
$ git clean -fd  
$ git push --force-with-lease origin refactor/phase-2-5:abort (optional)  

For partial rollback per step, use interactive rebase:

$ git reset --soft <commit-sha-before-step>  
Fix & recommit.

────────────────────────────────────────────────────────────────────────
Estimated time: 4–6 engineer hours or ~25 ChatGPT calls.
