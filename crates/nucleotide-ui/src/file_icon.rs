// ABOUTME: Shared file icon component using SVG assets for consistent UI
// ABOUTME: Provides type-aware icons for files and folders across the application

use gpui::{Hsla, IntoElement, Styled, Svg, svg};
use std::path::Path;

use crate::tokens::STANDARD_ICON_SIZE;

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
            size: STANDARD_ICON_SIZE,
            color: None,
        }
    }

    /// Create a file icon from extension string
    pub fn from_extension(extension: Option<&str>) -> Self {
        Self {
            extension: extension.map(|s| s.to_lowercase()),
            is_directory: false,
            is_expanded: false,
            size: STANDARD_ICON_SIZE,
            color: None,
        }
    }

    /// Create a directory icon
    pub fn directory(is_expanded: bool) -> Self {
        Self {
            extension: None,
            is_directory: true,
            is_expanded,
            size: STANDARD_ICON_SIZE,
            color: None,
        }
    }

    /// Create an icon for a scratch buffer (unnamed file)
    pub fn scratch() -> Self {
        Self {
            extension: Some("scratch".to_string()), // Use special marker for scratch buffers
            is_directory: false,
            is_expanded: false,
            size: STANDARD_ICON_SIZE,
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
            size: STANDARD_ICON_SIZE,
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

    fn icon_path(&self) -> &'static str {
        if self.is_directory {
            if self.is_expanded {
                "icons/folder-open.svg"
            } else {
                "icons/folder.svg"
            }
        } else {
            match self.extension.as_deref() {
                Some("c") => "icons/file-c.svg",
                Some("cs") => "icons/file-c-sharp.svg",
                Some("cpp" | "cc" | "cxx") => "icons/file-cpp.svg",
                Some("css") => "icons/file-css.svg",
                Some("csv") => "icons/file-csv.svg",
                Some("doc" | "docx") => "icons/file-doc.svg",
                Some("html" | "htm") => "icons/file-html.svg",
                Some("ini") => "icons/file-ini.svg",
                Some("jpg" | "jpeg") => "icons/file-jpg.svg",
                Some("js") => "icons/file-js.svg",
                Some("jsx") => "icons/file-jsx.svg",
                Some("md" | "markdown") => "icons/file-md.svg",
                Some("pdf") => "icons/file-pdf.svg",
                Some("png") => "icons/file-png.svg",
                Some("ppt" | "pptx") => "icons/file-ppt.svg",
                Some("py") => "icons/file-py.svg",
                Some("rs") => "icons/file-rs.svg",
                Some("sql") => "icons/file-sql.svg",
                Some("svg") => "icons/file-svg.svg",
                Some("ts") => "icons/file-ts.svg",
                Some("tsx") => "icons/file-tsx.svg",
                Some("txt") => "icons/file-txt.svg",
                Some("vue") => "icons/file-vue.svg",
                Some("xls" | "xlsx") => "icons/file-xls.svg",
                Some("zip") => "icons/file-zip.svg",
                Some("json") => "icons/file-json.svg",
                Some("gif" | "bmp" | "ico" | "webp") => "icons/file-image.svg",
                Some("tar" | "gz" | "rar" | "7z" | "bz2" | "xz") => "icons/file-archive.svg",
                Some("sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd") => {
                    "icons/file-terminal.svg"
                }
                Some("lock") => "icons/file-lock.svg",
                Some("git" | "gitignore" | "gitattributes") => "icons/git-branch.svg",
                Some("link") => "icons/link.svg",
                Some("link-broken") => "icons/link-broken.svg",
                Some("scratch") => "icons/file-question.svg",
                Some(
                    "astro" | "cjs" | "clj" | "cljs" | "cljc" | "cmake" | "coffee" | "conf"
                    | "config" | "dart" | "env" | "ex" | "exs" | "fs" | "fsi" | "fsx" | "gleam"
                    | "go" | "gql" | "graphql" | "groovy" | "h" | "hh" | "hpp" | "hs" | "hxx"
                    | "java" | "jl" | "jsonc" | "kt" | "kts" | "lua" | "m" | "mm" | "nix" | "php"
                    | "pl" | "pm" | "proto" | "r" | "rb" | "rkt" | "scala" | "scm" | "sol"
                    | "swift" | "tf" | "tfvars" | "toml" | "vala" | "v" | "vb" | "wasm" | "wgsl"
                    | "xml" | "yaml" | "yml" | "zig",
                ) => "icons/file-code.svg",
                Some("adoc" | "asc" | "log" | "nfo" | "org" | "rst" | "rtf" | "tex" | "text") => {
                    "icons/file-text.svg"
                }
                None | Some(_) => "icons/file.svg",
            }
        }
    }

    /// Get the appropriate SVG for this file type.
    fn get_svg(&self) -> Svg {
        svg().path(self.icon_path())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_phosphor_extension_icon() {
        let mappings = [
            ("c", "file-c"),
            ("cs", "file-c-sharp"),
            ("cpp", "file-cpp"),
            ("css", "file-css"),
            ("csv", "file-csv"),
            ("doc", "file-doc"),
            ("html", "file-html"),
            ("ini", "file-ini"),
            ("jpg", "file-jpg"),
            ("js", "file-js"),
            ("jsx", "file-jsx"),
            ("md", "file-md"),
            ("pdf", "file-pdf"),
            ("png", "file-png"),
            ("ppt", "file-ppt"),
            ("py", "file-py"),
            ("rs", "file-rs"),
            ("sql", "file-sql"),
            ("svg", "file-svg"),
            ("ts", "file-ts"),
            ("tsx", "file-tsx"),
            ("txt", "file-txt"),
            ("vue", "file-vue"),
            ("xls", "file-xls"),
            ("zip", "file-zip"),
        ];

        for (extension, icon) in mappings {
            assert_eq!(
                FileIcon::from_extension(Some(extension)).icon_path(),
                format!("icons/{icon}.svg")
            );
        }
    }

    #[test]
    fn maps_extension_aliases_to_their_phosphor_icons() {
        let mappings = [
            ("cc", "file-cpp"),
            ("cxx", "file-cpp"),
            ("docx", "file-doc"),
            ("htm", "file-html"),
            ("jpeg", "file-jpg"),
            ("markdown", "file-md"),
            ("pptx", "file-ppt"),
            ("xlsx", "file-xls"),
        ];

        for (extension, icon) in mappings {
            assert_eq!(
                FileIcon::from_extension(Some(extension)).icon_path(),
                format!("icons/{icon}.svg")
            );
        }
    }

    #[test]
    fn uses_semantic_fallbacks_for_unmapped_extensions() {
        assert_eq!(
            FileIcon::from_extension(Some("go")).icon_path(),
            "icons/file-code.svg"
        );
        assert_eq!(
            FileIcon::from_extension(Some("log")).icon_path(),
            "icons/file-text.svg"
        );
        assert_eq!(
            FileIcon::from_extension(Some("text")).icon_path(),
            "icons/file-text.svg"
        );
        assert_eq!(
            FileIcon::from_extension(Some("unknown")).icon_path(),
            "icons/file.svg"
        );
        assert_eq!(FileIcon::from_extension(None).icon_path(), "icons/file.svg");
    }
}
