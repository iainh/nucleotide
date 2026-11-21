# Icons

## Platform rendering

Use `nucleotide_ui::icon().path("icons/plus.svg")` for application icons,
or the shared `FileIcon` component for files and folders. The path is a stable
semantic identifier; it no longer requires SVG rendering on every platform.

- **Windows:** installed Segoe Fluent Icons glyphs, with Segoe MDL2 Assets as
  the Windows 10 fallback. GPUI renders them through DirectWrite at 80% of the
  icon slot size, rounded to a logical pixel and centred without changing layout
  or click targets. Icons inherit the control's colour, including hover and
  disabled states.
- **macOS:** runtime SF Symbols through AppKit, rasterized at the display scale
  with the requested tint. A bounded cache retains only owned image pixels,
  never native objects. Native traffic lights remain managed by macOS.
- **Linux:** the bundled artwork below, for every icon.

The resolver lives in `crates/nucleotide-ui/src/icon/`. Add native equivalents
in `symbols.rs`, not in individual controls. Unmapped names, unavailable symbols
or fonts, and transformed artwork use the bundled SVG. Specialised completion
badges, VCS and diff-layout imagery and the animated spinner deliberately
retain their artwork.
Code-file extensions share the platform's code symbol rather than lettered
extension badges. These are semantic system symbols, not Explorer/Finder file
association icons or thumbnails; rendering never queries file associations.

Keep `gpui::svg()` for actual SVG documents and diagrams, not UI icons. Do not
bundle either platform's icon font or export SF Symbols as replacement assets.
Native rendering applies in both system and theme UI looks.

## Bundled artwork

All assets in this directory come from [Pierre Icons](https://github.com/pierrecomputer/icons)
by The Pierre Computer Company. They are the 16px outline artwork from the
beta collection (`beta/svg/Icon<Name>/outline-16px.svg`), with a few from the
stable collection (`svg/Icon<Name>.svg`) where beta has no equivalent. They are
renamed to keep the stable `icons/*.svg` paths used by call sites, minified to
one line, and stable-collection fills are changed from `black` to
`currentColor`.

Source version: commit `aae3cf5` (2026-11-20).

Stable-collection sources: `braces` (Braces), `completion-function` (Function),
`completion-method` (Parentheses), `completion-module` (BoxTape),
`completion-snippet` and `square-scissors` (CodeBlock), `completion-variable`
(Variable), `monitor` (Desktop), `more-horizontal` (Ellipsis), `pin` (Pin),
`refresh` and `rotate-ccw` (Reload).

Pierre has no artwork for these, so they are derived from Pierre geometry:

- `chevron-right` and `chevron-up`: Chevron, rotated.
- `play`: Triangle, rotated to point right.
- `rotate-ccw`: Reload, mirrored.
- `zoom-in` and `zoom-out`: MagnifyingGlass with a plus or minus drawn at
  Pierre's 1.25-unit stroke.

`diff-split` and `diff-unified` (DiffSplit and DiffStacked) are scaled to 80%
around their centre so their solid tiles don't crowd button labels.

Pierre has no per-language file icons. Extension-specific names such as
`file-rs.svg` keep their paths but share FileCode, FileText, FileImage,
FileRichText, FileSpreadsheet or FileZip artwork, matching the native symbols.

## License

Licensed under the Apache License, Version 2.0 (the "License"); you may not
use these files except in compliance with the License. You may obtain a copy
of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS, WITHOUT
WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the
License for the specific language governing permissions and limitations under
the License.

Copyright The Pierre Computer Company. Modified as described above.
