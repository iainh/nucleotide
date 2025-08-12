────────────────────────────────────────────────────────────────────────
IMPLEMENTATION PLAN – NUCLEOTIDE ARCHITECTURAL HARDENING (PHASE 3.0)
────────────────────────────────────────────────────────────────────────
Legend  
• $ denotes shell command from repository root  
• ⇒ means "expect this result"  
• <…> placeholders must be replaced literally in code  
• All file paths are absolute from workspace root unless otherwise noted

Target finish criteria  
✓ All helix-* crates are behind optional feature helix-integration, OFF by default  
✓ crates/nucleotide is split into crates/nucleotide-app (library) + crates/nucleotide-bin (binary)  
✓ nucleotide-ui no longer directly references Workspace types; communication happens solely through trait-based bridge crates  
✓ CI enforces:  
 • no cross-layer deps (cargo-deny tree rules)  
 • helix-integration remains optional and unused in release builds  
✓ Updated docs & READMEs; layer diagram reflects new crates  
✓ Rollback tag pre-phase-3 exists, hard reset documented

High-level steps  
1. Branch & safety net  
2. Helix dependency gating  
3. Complete Workspace ⇄ UI decoupling  
4. Split main crate for alternative front-ends  
5. CI hardening & scripts  
6. Docs / verification  
7. Rollback instructions

────────────────────────────────────────────────────────────────────────
1. Branch & safety net
────────────────────────────────────────────────────────────────────────
1.1 $ git checkout main && git pull  
1.2 $ git checkout -b refactor/phase-3  
1.3 $ git tag -s pre-phase-3 -m "Checkpoint before architectural-hardening phase 3"

────────────────────────────────────────────────────────────────────────
2. Gate helix dependencies behind feature
────────────────────────────────────────────────────────────────────────
Current state: crates/nucleotide-editor and maybe others depend on helix-core, helix-unicode-segmentation, etc.

2.1 Modify each Cargo.toml that lists helix-* deps:

# Example: crates/nucleotide-editor/Cargo.toml
[features]
default = []
helix-integration = ["helix-core", "helix-unicode-segmentation"]

[dependencies]
helix-core = { workspace = true, optional = true }
helix-unicode-segmentation = { workspace = true, optional = true }

2.2 Update code that unconditionally uses helix symbols:

// BEFORE
use helix_core::text::Rope;
pub fn translate_rope(r: &Rope) { … }

// AFTER (crates/nucleotide-editor/src/rope_adapter.rs)
#[cfg(feature = "helix-integration")]
use helix_core::text::Rope;
#[cfg(feature = "helix-integration")]
pub fn translate_rope(r: &Rope) { … }

Provide harmless stubs when feature is absent:

#[cfg(not(feature = "helix-integration"))]
pub struct RopeDummy;
#[cfg(not(feature = "helix-integration"))]
pub fn translate_rope(_: &RopeDummy) { /* noop */ }

2.3 Add compile-time guard test:

crates/nucleotide-editor/tests/feature_gates.rs
#[test]
fn helix_disabled_compiles() {
    #[cfg(feature = "helix-integration")]
    panic!("run cargo test without helix-integration to verify gate");
}

2.4 Verify:

$ cargo check -p nucleotide-editor                   # default features
⇒ OK, helix not compiled

$ cargo check -p nucleotide-editor --features helix-integration
⇒ OK, helix compiled

────────────────────────────────────────────────────────────────────────
3. Finish Workspace ⇄ UI decoupling
────────────────────────────────────────────────────────────────────────
Goal: UI must not import Workspace structs directly; editor talks to UI through bridge traits created in phase 2.5.

3.1 Search for remaining violations:

$ rg --glob "crates/nucleotide-ui/**/*" "WorkspaceManager|PanelId|TabGroup"

If any, refactor to use capability traits defined in nucleotide-core::capabilities or add missing ones.

Example trait addition (crates/nucleotide-core/src/capabilities/workspace.rs):

pub trait WorkspaceQuery {
    fn active_tab_id(&self) -> TabId;
    fn set_active_tab(&mut self, TabId);
}

Implementation in bridge (crates/nucleotide-ui/src/bridge/workspace.rs):

impl WorkspaceQuery for crate::UiBridge { … }

3.2 Remove direct dependency in Cargo.toml:

crates/nucleotide-ui/Cargo.toml  
- nucleotide-workspace = { path = "../nucleotide-workspace" }

3.3 cargo check ‑p nucleotide-ui

────────────────────────────────────────────────────────────────────────
4. Split crates/nucleotide into app + CLI
────────────────────────────────────────────────────────────────────────
Rationale – a GTK or TUI frontend should link to nucleotide-app without dragging the gpui runtime.

4.1 Create new library crate:

$ cargo new crates/nucleotide-app --lib
Edit crates/nucleotide-app/Cargo.toml:

[dependencies]
nucleotide-editor = { path = "../nucleotide-editor" }
nucleotide-ui = { path = "../nucleotide-ui", optional = true }
gpui = { workspace = true, optional = true }

[features]
default = ["gpui-app"]
gpui-app = ["gpui", "nucleotide-ui"]

4.2 Move application glue code:

mv crates/nucleotide/src/lib.rs crates/nucleotide-app/src/lib.rs
mv crates/nucleotide/src/main.rs crates/nucleotide-bin/src/main.rs  # after we create bin

Remove gpui-specific constructs from lib.rs behind gpui-app feature:

#[cfg(feature = "gpui-app")]
pub fn run_gpui() { … }

4.3 Create binary crate for default GUI:

$ cargo new crates/nucleotide-bin --bin
Edit crates/nucleotide-bin/Cargo.toml:

[dependencies]
nucleotide-app = { path = "../nucleotide-app", features = ["gpui-app"] }

Main file crates/nucleotide-bin/src/main.rs:

fn main() {
    env_logger::init();
    nucleotide_app::run_gpui();
}

4.4 Update workspace Cargo.toml members.

4.5 Ensure previous "nucleotide" name is reserved:

Add to root Cargo.toml
[patch.crates-io]
nucleotide = { path = "crates/nucleotide-bin" }

(or rename crate on crates.io later)

4.6 Verify:

$ cargo run -p nucleotide-bin  
⇒ app launches

$ cargo check -p nucleotide-app --no-default-features  
⇒ compiles without gpui

────────────────────────────────────────────────────────────────────────
5. CI hardening & scripts
────────────────────────────────────────────────────────────────────────
We extend phase 2.5 CI.

5.1 Update .github/workflows/ci.yml (jobs.build.steps):

- run: cargo deny check
- run: cargo udeps --all-targets --workspace
- run: scripts/check-layering.sh
- run: cargo check --workspace --all-targets --no-default-features
- run: cargo check -p nucleotide-editor --features helix-integration
- run: cargo check -p nucleotide-app --features gpui-app

5.2 Enhance scripts/check-layering.sh:

#!/usr/bin/env bash
set -euo pipefail
# forbid helix deps without feature
if cargo tree -e normal -p nucleotide-editor | grep -q "helix-core"; then
  echo "ERROR: helix-core present without helix-integration feature"
  exit 1
fi
# forbid ui in editor
cargo tree -e normal -i nucleotide-editor | grep "nucleotide-ui" && { echo "Layer violation"; exit 1; }

5.3 Add cargo-hack matrix in CI (fast feature combos):

- run: cargo hack check --feature-powerset --depth 2

────────────────────────────────────────────────────────────────────────
6. Docs & verification matrix
────────────────────────────────────────────────────────────────────────
6.1 Update LAYERED_ARCHITECTURE.md (new crate graph).  
6.2 Each new crate gets README.md with purpose & feature table.  
6.3 Run:

$ cargo test --workspace  
$ cargo clippy --all-targets -- -D warnings  
$ cargo deny check  
Manual: launch bin, open file, check completion.

6.4 Publish-dry-run:

$ cargo publish --dry-run -p nucleotide-types

────────────────────────────────────────────────────────────────────────
7. Rollback procedure
────────────────────────────────────────────────────────────────────────
If any stage fails:

$ git reset --hard pre-phase-3  
$ git clean -fd  
$ git push --force-with-lease origin refactor/phase-3:abort  # optional

For partial rollback:

$ git reset --soft <sha_before_step>  
Fix issues, recommit, force-push branch.

────────────────────────────────────────────────────────────────────────
Estimated effort: 4-6 engineer hours (~20 ChatGPT calls)
────────────────────────────────────────────────────────────────────────
