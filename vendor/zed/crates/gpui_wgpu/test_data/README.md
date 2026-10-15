# Ligature test font

`ligature.ttf` is an original synthetic font, not a subset of a third-party font.
Its glyphs are rectangles. It has 1000 units per em and these advances:

| Glyph | Advance |
| --- | --- |
| `.notdef`, `m`, `f` | 600 |
| `i` | 300 |
| `fi` | 850 |
| `é` | 500 |

The `liga` feature substitutes `f i` with `fi`. Unequal advances let tests detect
feature overrides that are ignored, as well as incorrect concatenation offsets.
The font is covered by this crate's Apache-2.0 licence.
