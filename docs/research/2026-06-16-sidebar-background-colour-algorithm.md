# Research: Sidebar Background Colour Algorithm

**Date**: 2026-06-16
**Question**: What should the sidebar/file-tree background be relative to the titlebar and editor background?
**Status**: Complete

## Context

The current file tree background is a chrome token. `FileTreeView` reads
`theme.tokens.file_tree_tokens().background` and applies it directly to the
container, so the algorithm belongs in `nucleotide-ui`, not in file-tree
rendering.

Relevant code paths:

- `crates/nucleotide/src/file_tree/view.rs:1323` reads `FileTreeTokens`.
- `crates/nucleotide-ui/src/tokens/mod.rs:927` maps file tree background to
  `chrome.file_tree_background`.
- `crates/nucleotide-ui/src/tokens/mod.rs:730` receives both
  `surface_color` and `editor_background`, but currently passes only
  `surface_color` into `ChromeTokens::from_surface_color`.
- `crates/nucleotide-ui/src/styling/color_theory.rs:625` derives titlebar,
  footer, file tree, tab empty, and separator colours from one surface.

Current behaviour:

- Dark themes: file tree background equals titlebar background.
- Light themes: file tree background is closer to the base chrome surface than
  the titlebar, but it is not explicitly derived from editor/titlebar relation.

This makes the dark sidebar read as top chrome, not as a supporting navigation
pane.

## Findings

### Platform Guidance

Apple and Windows both push toward layered UI hierarchy rather than one flat
chrome colour.

Apple's public design tips emphasize readable contrast, visual grouping, and not
relying on colour alone. For this sidebar decision, that means the file tree
must keep enough text contrast and should not use colour as the only separator.
The Apple HIG colour/material/sidebar pages are JavaScript-rendered, but their
platform convention is consistent with this: sidebars are supporting surfaces
with material/visual separation from content, not merely copies of titlebar
chrome.

Microsoft's Windows material guidance is more explicit. Mica is the base
window material, Mica Alt is suited to apps with a tabbed titlebar, and Windows
layering separates base layer, content layer, and in-app layers. The sidebar is
closer to a navigation/content-support layer than to top-level titlebar chrome.

Implication: the sidebar should sit between editor content and titlebar chrome,
with its own token. It should not collapse into either surface.

### Colour Theory

The midpoint idea is directionally right, but the midpoint should be perceptual,
not HSL-linear or RGB-linear:

- Use OKLab/OKLCH because the existing code already uses OKLab for lightness
  shifts and OKLCH for token `mix`.
- Blend from the editor background toward the titlebar background.
- Keep saturation bounded by the existing chrome neutralization so strong theme
  hues do not dominate a large sidebar.
- Preserve accessible text contrast by deriving text after the background, or
  by verifying existing `chrome.text_on_chrome` still satisfies WCAG AA.

The sidebar is a persistent navigation surface. It should be visibly distinct
from the editor, but lower in visual weight than the titlebar. A literal 50/50
mix is acceptable, but a slightly editor-biased mix usually feels quieter in an
editor because the sidebar is large and continuously visible.

Recommended ratio:

```text
sidebar_background = mix_oklch(editor_background, titlebar_background, 0.45)
```

This means:

- `0.0` is editor background.
- `1.0` is titlebar background.
- `0.45` lands just under halfway toward titlebar, giving the sidebar a clear
  supporting-surface identity without matching top chrome.

For themes where that result is too close to the editor, nudge toward the
titlebar until the editor/sidebar contrast reaches a minimum visual distinction
threshold. Reuse the existing `CHROME_MIN_CONTRAST` of `1.2`.

### Current Code Fit

The clean implementation is to let chrome derivation know the editor background:

```rust
pub fn derive_chrome_colors(surface_color: Hsla, editor_background: Hsla) -> ChromeColors
```

Then compute titlebar as today from `surface_color`, and compute the file tree
from editor/titlebar:

```rust
let mut ratio = 0.45;
let mut file_tree_background =
    ColorTheory::mix_oklch(editor_background, titlebar_background, ratio);

while ColorTheory::contrast_ratio(editor_background, file_tree_background) < CHROME_MIN_CONTRAST {
    ratio = (ratio + 0.05).min(0.70);
    file_tree_background =
        ColorTheory::mix_oklch(editor_background, titlebar_background, ratio);
    if ratio >= 0.70 {
        break;
    }
}
```

Keep:

- `footer_background = titlebar_background`
- `tab_empty_background = file_tree_background`
- separators and borders as explicit separators, because Apple guidance warns
  against relying on colour alone and because low-contrast layer differences can
  be subtle on calibrated or translucent displays.

`ChromeTokens::from_surface_color` should either become:

```rust
pub fn from_surface_color(surface_color: Hsla, editor_background: Hsla, is_dark: bool) -> Self
```

or gain a sibling constructor that takes editor background. Since
`DesignTokens::from_helix_and_surface` already has the editor background, passing
it through is a small, direct API change.

## Options Considered

| Option | Pros | Cons | Effort |
|--------|------|------|--------|
| Keep current algorithm | No code change; already tested | Dark sidebar equals titlebar; weak hierarchy | None |
| Literal 50/50 perceptual midpoint | Matches the initial idea; easy to explain | Can be slightly too assertive on large sidebars | Low |
| Editor-biased perceptual midpoint at 0.45 with contrast floor | Better hierarchy; quieter large sidebar; fits platform layering | Slightly more logic and tests | Low |
| Use theme `ui.menu` for sidebar | Respects some theme authors' panel colour | Previous code comments warn not to use menu for base chrome; can clash with editor | Medium |
| Platform-specific material/translucency | Closer to native macOS/Windows material systems | Much broader rendering and platform work; harder to test | High |

## Recommendation

Use an editor-biased perceptual midpoint:

1. Continue deriving titlebar/footer from the chrome surface.
2. Derive sidebar/file-tree background by OKLCH mixing editor background toward
   titlebar background at `0.45`.
3. Increase the mix ratio up to `0.70` only if the sidebar/editor contrast is
   below `CHROME_MIN_CONTRAST`.
4. Keep tab empty background aligned with sidebar.
5. Keep borders/separators active, because colour alone is not a reliable
   boundary.

This is more defensible than making the sidebar exactly midway in raw colour
space. It respects the user's intuition, but uses perceptual mixing and a
minimum distinction threshold so the result survives both dark and light themes.

## Test Plan

Update or add tests near `derive_chrome_colors` and component token tests:

- Dark theme: file tree background must not equal titlebar background.
- Light theme: file tree background must not equal editor/background surface or
  titlebar background.
- Both themes: file tree lightness is between editor background and titlebar
  background in OKLab `L`.
- Both themes: file tree/editor contrast is at least `CHROME_MIN_CONTRAST` when
  the titlebar allows it.
- File tree text contrast remains at least WCAG AA through
  `tokens.file_tree_tokens()`.
- Tab empty background equals file tree background.

Likely command:

```bash
cargo test -p nucleotide-ui --lib tokens
```

If the implementation touches only `nucleotide-ui`, a faster first check is:

```bash
cargo test -p nucleotide-ui test_chrome_colors
```

## Open Questions

- Should titlebar remain the same for dark themes, or should titlebar/footer
  also become slightly less lifted once sidebar is quieter? I would leave it
  unchanged initially to keep the change scoped.
- Should `ui.menu` ever opt into sidebar background? Current code comments say
  not to fall back to `ui.menu` for base surface extraction, so this should be a
  separate theme-authoring decision.

## References

- Apple UI Design Dos and Don'ts: https://developer.apple.com/design/tips/
- Apple Human Interface Guidelines, Colour: https://developer.apple.com/design/human-interface-guidelines/color
- Apple Human Interface Guidelines, Materials: https://developer.apple.com/design/human-interface-guidelines/materials
- Apple Human Interface Guidelines, Sidebars: https://developer.apple.com/design/human-interface-guidelines/sidebars
- Microsoft, Materials in Windows apps: https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/materials
- Microsoft, Layering guidance: https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/layering
- Microsoft, Mica material: https://learn.microsoft.com/en-us/windows/apps/design/style/mica
- Current implementation:
  - `crates/nucleotide-ui/src/styling/color_theory.rs:625`
  - `crates/nucleotide-ui/src/tokens/mod.rs:510`
  - `crates/nucleotide-ui/src/tokens/mod.rs:730`
  - `crates/nucleotide-ui/src/tokens/mod.rs:927`
  - `crates/nucleotide/src/file_tree/view.rs:1323`
