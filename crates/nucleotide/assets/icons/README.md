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
- **Linux:** the bundled artwork below.

The resolver lives in `crates/nucleotide-ui/src/icon/`. Add native equivalents
in `symbols.rs`, not in individual controls. Unmapped names, unavailable symbols
or fonts, and transformed artwork use the bundled SVG. Specialised completion
badges, VCS imagery and the animated spinner deliberately retain their artwork.
Code-file extensions share the platform's code symbol rather than lettered
extension badges. These are semantic system symbols, not Explorer/Finder file
association icons or thumbnails; rendering never queries file associations.

Keep `gpui::svg()` for actual SVG documents and diagrams, not UI icons. Do not
bundle either platform's icon font or export SF Symbols as replacement assets.
Native rendering applies in both system and theme UI looks.

## Bundled fallbacks

All assets in this directory are official [Phosphor Icons core](https://github.com/phosphor-icons/core)
icons. Most use the duotone weight, while compact action icons use the regular
weight for clarity. They are renamed locally where necessary to preserve the
stable `icons/*.svg` paths used by existing call sites. See the [Phosphor Icons
website](https://phosphoricons.com) for the icon catalog.

Source version: Phosphor Icons core v2.1.1, commit
`2b75f3ad12b420c9504ef05df8d2564a28f8500e`.

Extension-specific file icons retain their canonical `file-<extension>.svg`
names. Files without a dedicated extension icon use `file-code.svg` or
`file-text.svg` when their type is known, and `file.svg` otherwise.

## License

MIT License

Copyright (c) 2023 Phosphor Icons

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
