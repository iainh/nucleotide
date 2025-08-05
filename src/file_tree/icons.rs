// ABOUTME: Lucide icon system for file tree using SVG files from assets
// ABOUTME: Provides scalable vector icons for files, folders, and UI elements

use gpui::*;

/// Create a folder icon
pub fn folder_icon(open: bool) -> Svg {
    if open {
        svg().path("icons/folder-open.svg")
    } else {
        svg().path("icons/folder.svg")
    }
}

/// Create a file icon
pub fn file_icon() -> Svg {
    svg().path("icons/file.svg")
}

/// Create a chevron icon
pub fn chevron_icon(direction: &str) -> Svg {
    match direction {
        "down" => svg().path("icons/chevron-down.svg"),
        "right" => svg().path("icons/chevron-right.svg"),
        _ => svg().path("icons/chevron-right.svg"),
    }
}

/// Get icon for file type
pub fn get_file_icon(extension: Option<&str>, is_directory: bool, is_expanded: bool) -> Svg {
    if is_directory {
        folder_icon(is_expanded)
    } else {
        match extension {
            Some("rs" | "js" | "ts" | "py" | "go" | "java" | "cpp" | "c" | "h" | "cs" |
            "php" | "rb") => {
                svg().path("icons/code.svg")
            }
            Some("json" | "yaml" | "yml" | "xml") => {
                svg().path("icons/braces.svg")
            }
            Some("md" | "txt" | "doc" | "docx" | "pdf") => {
                svg().path("icons/file-text.svg")
            }
            Some("toml" | "ini" | "conf" | "config" | "env") => {
                svg().path("icons/settings.svg")
            }
            _ => file_icon(),
        }
    }
}

/// Get symlink icon based on target existence
pub fn get_symlink_icon(target_exists: bool) -> Svg {
    if target_exists {
        svg().path("icons/link.svg")
    } else {
        svg().path("icons/link-broken.svg")
    }
}