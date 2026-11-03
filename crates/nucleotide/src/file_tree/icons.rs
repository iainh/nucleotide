// ABOUTME: File-tree navigation icons that are not file-type specific
// ABOUTME: File and folder imagery is owned by nucleotide-ui FileIcon

use nucleotide_ui::{Icon as Svg, icon as svg};

/// Create a chevron icon
pub fn chevron_icon(direction: &str) -> Svg {
    match direction {
        "down" => svg().path("icons/chevron-down.svg"),
        "right" => svg().path("icons/chevron-right.svg"),
        _ => svg().path("icons/chevron-right.svg"),
    }
}
