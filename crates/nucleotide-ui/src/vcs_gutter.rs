// ABOUTME: VCS gutter component for rendering diff indicators in the editor gutter
// ABOUTME: Integrates with design tokens for modern, theme-aware VCS diff visualization

use crate::tokens::DesignTokens;
use gpui::{Div, IntoElement, Pixels, Styled, div, prelude::*, px};
use nucleotide_types::{DiffChangeType, DiffHunkInfo};

/// VCS gutter component that renders diff indicators
#[derive(Debug, Clone)]
pub struct VcsGutter {
    /// Diff hunks to render
    hunks: Vec<DiffHunkInfo>,
    /// Line height for proper positioning
    line_height: Pixels,
    /// Gutter width
    gutter_width: Pixels,
}

impl VcsGutter {
    /// Create a new VCS gutter
    pub fn new(hunks: Vec<DiffHunkInfo>, line_height: Pixels, gutter_width: Pixels) -> Self {
        Self {
            hunks,
            line_height,
            gutter_width,
        }
    }

    /// Render the VCS gutter indicators
    pub fn render(&self, design_tokens: &DesignTokens) -> Div {
        div().w(self.gutter_width).children(
            self.hunks
                .iter()
                .enumerate()
                .map(|(i, hunk)| self.render_hunk_indicator(hunk, i, design_tokens)),
        )
    }

    /// Render a single hunk indicator
    fn render_hunk_indicator(
        &self,
        hunk: &DiffHunkInfo,
        _index: usize,
        design_tokens: &DesignTokens,
    ) -> impl IntoElement {
        let (color, symbol) = match hunk.change_type {
            DiffChangeType::Addition => (design_tokens.editor.vcs_added, "▍"),
            DiffChangeType::Modification => (design_tokens.editor.vcs_modified, "▍"),
            DiffChangeType::Deletion => (design_tokens.editor.vcs_deleted, "▔"),
        };

        // Calculate position based on line numbers
        let start_line = hunk.after_start;
        let end_line = hunk.after_end;
        let top_offset = px(start_line as f32 * self.line_height.0);

        // For deletions, we show a single line indicator
        // For additions/modifications, we show indicators for the affected lines
        let height = if hunk.change_type == DiffChangeType::Deletion {
            self.line_height
        } else {
            px((end_line - start_line) as f32 * self.line_height.0)
        };

        div()
            .absolute()
            .top(top_offset)
            .left(px(0.0))
            .w(px(4.0)) // Indicator width
            .h(height)
            .bg(color)
            .child(div().size_full().text_color(color).child(symbol))
    }

    /// Get the diff indicator for a specific line (0-indexed)
    pub fn get_line_indicator(
        &self,
        line: usize,
        design_tokens: &DesignTokens,
    ) -> Option<VcsLineIndicator> {
        for hunk in &self.hunks {
            let line_u32 = line as u32;

            match hunk.change_type {
                DiffChangeType::Addition | DiffChangeType::Modification => {
                    if line_u32 >= hunk.after_start && line_u32 < hunk.after_end {
                        let color = match hunk.change_type {
                            DiffChangeType::Addition => design_tokens.editor.vcs_added,
                            DiffChangeType::Modification => design_tokens.editor.vcs_modified,
                            _ => unreachable!(),
                        };
                        return Some(VcsLineIndicator {
                            symbol: "▍",
                            color,
                            change_type: hunk.change_type,
                        });
                    }
                }
                DiffChangeType::Deletion => {
                    // Show deletion indicator on the line where content was removed
                    if line_u32 == hunk.after_start {
                        return Some(VcsLineIndicator {
                            symbol: "▔",
                            color: design_tokens.editor.vcs_deleted,
                            change_type: hunk.change_type,
                        });
                    }
                }
            }
        }
        None
    }
}

/// Information about a VCS indicator for a specific line
#[derive(Debug, Clone)]
pub struct VcsLineIndicator {
    /// Symbol to display (▍ for additions/modifications, ▔ for deletions)
    pub symbol: &'static str,
    /// Color for the indicator
    pub color: gpui::Hsla,
    /// Type of change
    pub change_type: DiffChangeType,
}

impl VcsLineIndicator {
    /// Render this indicator as a gutter element
    pub fn render(&self, line_height: Pixels) -> impl IntoElement {
        div()
            .w(px(4.0))
            .h(line_height)
            .bg(self.color)
            .child(div().size_full().text_color(self.color).child(self.symbol))
    }

    /// Create a VCS line indicator component
    pub fn component(symbol: &'static str, color: gpui::Hsla, change_type: DiffChangeType) -> Self {
        Self {
            symbol,
            color,
            change_type,
        }
    }
}

/// Helper function to get VCS indicator for a line from hunks
pub fn get_vcs_indicator_for_line(
    line: usize,
    hunks: &[DiffHunkInfo],
    design_tokens: &DesignTokens,
) -> Option<VcsLineIndicator> {
    let line_u32 = line as u32;

    for hunk in hunks {
        match hunk.change_type {
            DiffChangeType::Addition | DiffChangeType::Modification => {
                if line_u32 >= hunk.after_start && line_u32 < hunk.after_end {
                    let color = match hunk.change_type {
                        DiffChangeType::Addition => design_tokens.editor.vcs_added,
                        DiffChangeType::Modification => design_tokens.editor.vcs_modified,
                        _ => unreachable!(),
                    };
                    return Some(VcsLineIndicator {
                        symbol: "▍",
                        color,
                        change_type: hunk.change_type,
                    });
                }
            }
            DiffChangeType::Deletion => {
                // Show deletion indicator on the line where content was removed
                if line_u32 == hunk.after_start {
                    return Some(VcsLineIndicator {
                        symbol: "▔",
                        color: design_tokens.editor.vcs_deleted,
                        change_type: hunk.change_type,
                    });
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::{BaseColors, ChromeTokens, DesignTokens, EditorTokens};

    fn test_design_tokens() -> DesignTokens {
        DesignTokens {
            editor: EditorTokens::fallback(false),
            chrome: ChromeTokens::fallback(false),
            colors: crate::tokens::SemanticColors::from_base_light(&BaseColors::light()),
            sizes: crate::tokens::SizeTokens::default(),
        }
    }

    #[test]
    fn test_vcs_gutter_creation() {
        let hunks = vec![
            DiffHunkInfo {
                after_start: 5,
                after_end: 8,
                before_start: 5,
                before_end: 5,
                change_type: DiffChangeType::Addition,
            },
            DiffHunkInfo {
                after_start: 15,
                after_end: 18,
                before_start: 12,
                before_end: 15,
                change_type: DiffChangeType::Modification,
            },
        ];

        let gutter = VcsGutter::new(hunks, px(20.0), px(40.0));
        assert_eq!(gutter.hunks.len(), 2);
        assert_eq!(gutter.line_height, px(20.0));
        assert_eq!(gutter.gutter_width, px(40.0));
    }

    #[test]
    fn test_line_indicator_detection() {
        let hunks = vec![
            DiffHunkInfo {
                after_start: 5,
                after_end: 8,
                before_start: 5,
                before_end: 5,
                change_type: DiffChangeType::Addition,
            },
            DiffHunkInfo {
                after_start: 10,
                after_end: 10,
                before_start: 8,
                before_end: 11,
                change_type: DiffChangeType::Deletion,
            },
        ];

        let tokens = test_design_tokens();

        // Test addition indicator
        let indicator = get_vcs_indicator_for_line(6, &hunks, &tokens);
        assert!(indicator.is_some());
        let indicator = indicator.unwrap();
        assert_eq!(indicator.symbol, "▍");
        assert_eq!(indicator.change_type, DiffChangeType::Addition);

        // Test deletion indicator
        let indicator = get_vcs_indicator_for_line(10, &hunks, &tokens);
        assert!(indicator.is_some());
        let indicator = indicator.unwrap();
        assert_eq!(indicator.symbol, "▔");
        assert_eq!(indicator.change_type, DiffChangeType::Deletion);

        // Test no indicator
        let indicator = get_vcs_indicator_for_line(0, &hunks, &tokens);
        assert!(indicator.is_none());
    }
}
