// ABOUTME: Shared file icon component using SVG assets for consistent UI
// ABOUTME: Provides type-aware icons for files and folders across the application

use gpui::{Hsla, IntoElement, Styled, Svg, svg};
use std::path::Path;

/// File icon component that provides consistent icons across the application
#[derive(Clone)]
pub struct FileIcon {
    /// File extension for determining icon type
    extension: Option<String>,
    /// Whether this represents a directory
    is_directory: bool,
    /// Whether directory is expanded (only relevant for directories)
    is_expanded: bool,
    /// Icon size in pixels
    size: f32,
    /// Icon color
    color: Option<Hsla>,
}

impl FileIcon {
    /// Create a new file icon from a file path
    pub fn from_path(path: &Path, is_expanded: bool) -> Self {
        let is_directory = path.is_dir();
        let extension = if is_directory {
            None
        } else {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|s| s.to_lowercase())
        };

        Self {
            extension,
            is_directory,
            is_expanded,
            size: 16.0, // Default size
            color: None,
        }
    }

    /// Create a file icon from extension string
    pub fn from_extension(extension: Option<&str>) -> Self {
        Self {
            extension: extension.map(|s| s.to_lowercase()),
            is_directory: false,
            is_expanded: false,
            size: 16.0,
            color: None,
        }
    }

    /// Create a directory icon
    pub fn directory(is_expanded: bool) -> Self {
        Self {
            extension: None,
            is_directory: true,
            is_expanded,
            size: 16.0,
            color: None,
        }
    }

    /// Create an icon for a scratch buffer (unnamed file)
    pub fn scratch() -> Self {
        Self {
            extension: Some("scratch".to_string()), // Use special marker for scratch buffers
            is_directory: false,
            is_expanded: false,
            size: 16.0,
            color: None,
        }
    }

    /// Create a symlink icon
    pub fn symlink(target_exists: bool) -> Self {
        Self {
            extension: Some(if target_exists {
                "link".to_string()
            } else {
                "link-broken".to_string()
            }),
            is_directory: false,
            is_expanded: false,
            size: 16.0,
            color: None,
        }
    }

    /// Set the icon size
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Set the icon color
    pub fn text_color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Get the appropriate SVG for this file type
    fn get_svg(&self) -> Svg {
        if self.is_directory {
            if self.is_expanded {
                svg().path("icons/folder-open.svg")
            } else {
                svg().path("icons/folder.svg")
            }
        } else {
            match self.extension.as_deref() {
                Some("rs") => svg().path("icons/file-code-2.svg"),
                Some("js" | "ts" | "jsx" | "tsx") => svg().path("icons/file-code-2.svg"),
                Some("py") => svg().path("icons/file-code-2.svg"),
                Some("go") => svg().path("icons/file-code-2.svg"),
                Some("java" | "kt") => svg().path("icons/file-code-2.svg"),
                Some("cpp" | "cc" | "cxx" | "c" | "h" | "hpp") => {
                    svg().path("icons/file-code-2.svg")
                }
                Some("cs") => svg().path("icons/file-code-2.svg"),
                Some("php") => svg().path("icons/file-code-2.svg"),
                Some("rb") => svg().path("icons/file-code-2.svg"),
                Some("swift") => svg().path("icons/file-code-2.svg"),
                Some("json") => svg().path("icons/file-json.svg"),
                Some("yaml" | "yml") => svg().path("icons/braces.svg"),
                Some("xml" | "html" | "htm") => svg().path("icons/braces.svg"),
                Some("md" | "markdown") => svg().path("icons/file-text.svg"),
                Some("txt") => svg().path("icons/file-text.svg"),
                Some("doc" | "docx" | "pdf") => svg().path("icons/file-text.svg"),
                Some("toml") => svg().path("icons/settings.svg"),
                Some("ini" | "conf" | "config") => svg().path("icons/settings.svg"),
                Some("env") => svg().path("icons/settings.svg"),
                Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "ico" | "webp") => {
                    svg().path("icons/file-image.svg")
                }
                Some("zip" | "tar" | "gz" | "rar" | "7z" | "bz2" | "xz") => {
                    svg().path("icons/file-archive.svg")
                }
                Some("sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd") => {
                    svg().path("icons/file-terminal.svg")
                }
                Some("lock") => svg().path("icons/file-text.svg"), // Lock files are text-like documents
                Some("git" | "gitignore" | "gitattributes") => svg().path("icons/git-branch.svg"),
                Some("link") => svg().path("icons/link.svg"),
                Some("link-broken") => svg().path("icons/link-broken.svg"),
                Some("scratch") => svg().path("icons/file-question.svg"), // Special case for scratch buffers
                None => svg().path("icons/file-text.svg"), // Extensionless files get document icon
                _ => svg().path("icons/file-text.svg"), // Generic document icon for any unmatched file
            }
        }
    }
}

impl IntoElement for FileIcon {
    type Element = Svg;

    fn into_element(self) -> Self::Element {
        let mut svg = self.get_svg().size(gpui::px(self.size)).flex_shrink_0(); // Don't shrink the icon

        if let Some(color) = self.color {
            svg = svg.text_color(color);
        }

        svg
    }
}
