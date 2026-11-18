// ABOUTME: Resolves links clicked in the rendered Markdown preview
// ABOUTME: Separates workspace files from URLs the system browser should open

use std::path::{Path, PathBuf};
use url::Url;

/// Where a clicked Markdown link leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarkdownLinkTarget {
    /// A file to open in the editor, with an optional 0-based line.
    File {
        path: PathBuf,
        line: Option<usize>,
    },
    /// A URL for the system handler, such as a browser or mail client.
    External(String),
    /// A heading anchor in the same document.
    Fragment(String),
    Unsupported,
}

/// Resolve `url` as written in the Markdown file at `markdown_path`.
///
/// Relative links resolve from the file's directory, as on GitHub. File links
/// may name a line as `path:12` or `path#L12`; other fragments are ignored.
pub(crate) fn resolve_markdown_link(url: &str, markdown_path: Option<&Path>) -> MarkdownLinkTarget {
    let url = url.trim();
    if url.is_empty() {
        return MarkdownLinkTarget::Unsupported;
    }
    if let Some(fragment) = url.strip_prefix('#') {
        return MarkdownLinkTarget::Fragment(fragment.to_string());
    }

    // `src/x.rs:12` would otherwise parse as a URL with the scheme `src/x.rs`.
    let (url, mut line) = split_line_suffix(url);
    if line.is_none()
        && let Ok(parsed) = Url::parse(url)
        && parsed.scheme() != "file"
    {
        return MarkdownLinkTarget::External(url.to_string());
    }

    let base = markdown_path.and_then(|path| Url::from_file_path(path).ok());
    let Ok(resolved) = Url::options().base_url(base.as_ref()).parse(url) else {
        return MarkdownLinkTarget::Unsupported;
    };
    // Percent-decoding here undoes the escaping the Markdown parser applied.
    let Ok(path) = resolved.to_file_path() else {
        return MarkdownLinkTarget::Unsupported;
    };
    if line.is_none() {
        line = resolved.fragment().and_then(fragment_line);
    }
    MarkdownLinkTarget::File { path, line }
}

/// Split a trailing `:N` (1-based) from a path-like link.
fn split_line_suffix(url: &str) -> (&str, Option<usize>) {
    if url.contains("://") {
        return (url, None);
    }
    let Some((path, suffix)) = url.rsplit_once(':') else {
        return (url, None);
    };
    // Require a path shape so `tel:123` stays a URL.
    let path_like = path.contains('/') || path.contains('.');
    match suffix.parse::<usize>() {
        Ok(line) if line >= 1 && path_like && suffix.bytes().all(|b| b.is_ascii_digit()) => {
            (path, Some(line - 1))
        }
        _ => (url, None),
    }
}

/// Parse GitHub-style line anchors such as `L12`, `L12-L20` and `L12C3`.
fn fragment_line(fragment: &str) -> Option<usize> {
    let digits = fragment.strip_prefix('L')?;
    let end = digits
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(digits.len());
    let line = digits[..end].parse::<usize>().ok()?;
    let rest = &digits[end..];
    let valid_rest = rest.is_empty() || rest.starts_with("-L") || rest.starts_with('C');
    (line >= 1 && valid_rest).then(|| line - 1)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn resolve(url: &str) -> MarkdownLinkTarget {
        resolve_markdown_link(url, Some(Path::new("/docs/README.md")))
    }

    fn file(path: &str, line: Option<usize>) -> MarkdownLinkTarget {
        MarkdownLinkTarget::File {
            path: PathBuf::from(path),
            line,
        }
    }

    #[test]
    fn file_links_resolve_from_the_markdown_directory() {
        for (url, expected) in [
            ("other.md", file("/docs/other.md", None)),
            ("guide/intro.md#intro", file("/docs/guide/intro.md", None)),
            ("../x.rs", file("/x.rs", None)),
            ("/abs/a.md", file("/abs/a.md", None)),
            ("file:///abs/path", file("/abs/path", None)),
            ("other.md?plain=1", file("/docs/other.md", None)),
            // The parser stores destinations percent-encoded.
            ("my%20file.md", file("/docs/my file.md", None)),
            ("%23hash.md", file("/docs/#hash.md", None)),
            ("caf%C3%A9.md", file("/docs/café.md", None)),
        ] {
            assert_eq!(resolve(url), expected, "{url}");
        }
    }

    #[test]
    fn file_links_can_name_a_line() {
        for (url, expected) in [
            ("x.rs:12", file("/docs/x.rs", Some(11))),
            ("src/x.rs#L12", file("/docs/src/x.rs", Some(11))),
            ("src/x.rs#L12-L20", file("/docs/src/x.rs", Some(11))),
            ("src/x.rs#L3C7", file("/docs/src/x.rs", Some(2))),
            // Not line anchors.
            ("x.rs#L0", file("/docs/x.rs", None)),
            ("x.rs#Lede", file("/docs/x.rs", None)),
        ] {
            assert_eq!(resolve(url), expected, "{url}");
        }
    }

    #[test]
    fn urls_with_other_schemes_open_externally_unchanged() {
        for url in [
            "https://a.b/c?d#e",
            "http://localhost:8080",
            "mailto:a@b.c",
            "vscode://file/x",
            "tel:123",
        ] {
            assert_eq!(
                resolve(url),
                MarkdownLinkTarget::External(url.into()),
                "{url}"
            );
        }
    }

    #[test]
    fn fragments_and_unresolvable_links() {
        assert_eq!(
            resolve("#demo"),
            MarkdownLinkTarget::Fragment("demo".into())
        );
        assert_eq!(resolve("  "), MarkdownLinkTarget::Unsupported);
        assert_eq!(
            resolve_markdown_link("other.md", None),
            MarkdownLinkTarget::Unsupported
        );
        assert_eq!(
            resolve_markdown_link("file:///abs", None),
            file("/abs", None)
        );
    }
}
