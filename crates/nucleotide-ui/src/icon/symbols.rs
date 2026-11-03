// ABOUTME: Semantic mappings to public SF Symbol names and Windows system-font codepoints.
// ABOUTME: Code-specific imagery without a native equivalent deliberately keeps its bundled fallback.

#[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
pub(super) struct Symbol {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub macos: &'static str,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub windows: char,
}

// Windows codepoints: Microsoft's Segoe Fluent Icons table. These established
// glyphs are also available in Segoe MDL2 Assets on Windows 10.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
pub(super) fn lookup(path: &str) -> Option<Symbol> {
    let name = path.strip_prefix("icons/")?.strip_suffix(".svg")?;
    let (macos, windows) = match name {
        "close" => ("xmark", '\u{e711}'),
        "window-minimize" => ("minus", '\u{e921}'),
        "window-maximize" => ("square", '\u{e922}'),
        "window-restore" => ("square.on.square", '\u{e923}'),
        "arrow-left" => ("arrow.left", '\u{e72b}'),
        "arrow-right" => ("arrow.right", '\u{e72a}'),
        "chevron-down" => ("chevron.down", '\u{e70d}'),
        "chevron-up" => ("chevron.up", '\u{e70e}'),
        "chevron-right" => ("chevron.right", '\u{e76c}'),
        "plus" => ("plus", '\u{e710}'),
        "pin" => ("pin", '\u{e718}'),
        "settings" => ("gearshape", '\u{e713}'),
        "search" => ("magnifyingglass", '\u{e721}'),
        "menu" => ("line.3.horizontal", '\u{e700}'),
        "terminal" | "file-terminal" => ("terminal", '\u{e756}'),
        "columns-2" => ("rectangle.split.2x1", '\u{e89a}'),
        "folder-tree" => ("sidebar.left", '\u{ed41}'),
        "folder" => ("folder", '\u{e8b7}'),
        "folder-open" => ("folder.fill", '\u{e838}'),
        "file" => ("doc", '\u{e8a5}'),
        "file-text" | "file-txt" | "file-md" | "file-doc" => ("doc.text", '\u{e8a5}'),
        "file-image" | "file-jpg" | "file-png" | "file-svg" => ("photo", '\u{e8b9}'),
        "file-archive" | "file-zip" => ("doc.zipper", '\u{f012}'),
        "file-lock" => ("lock", '\u{e72e}'),
        "file-question" => ("doc.badge.questionmark", '\u{e8a5}'),
        "file-pdf" => ("doc.richtext", '\u{ea90}'),
        "file-csv" | "file-xls" => ("tablecells", '\u{f0e2}'),
        "file-ppt" => ("rectangle.on.rectangle", '\u{e8a5}'),
        "file-code" | "code" | "file-c" | "file-c-sharp" | "file-cpp" | "file-css"
        | "file-html" | "file-ini" | "file-js" | "file-jsx" | "file-json" | "file-py"
        | "file-rs" | "file-sql" | "file-ts" | "file-tsx" | "file-vue" => {
            ("chevron.left.forwardslash.chevron.right", '\u{e943}')
        }
        "braces" => ("curlybraces", '\u{e943}'),
        "link" => ("link", '\u{e71b}'),
        "check" => ("checkmark", '\u{e73e}'),
        "square" => ("square", '\u{e739}'),
        "square-check-big" => ("checkmark.square", '\u{e73a}'),
        "circle-x" => ("xmark.circle", '\u{ea39}'),
        "triangle-alert" => ("exclamationmark.triangle", '\u{e7ba}'),
        "info" => ("info.circle", '\u{e946}'),
        "lightbulb" => ("lightbulb", '\u{ea80}'),
        "book-text" => ("book", '\u{e736}'),
        "download" => ("arrow.down.to.line", '\u{e896}'),
        "play" => ("play", '\u{e768}'),
        "rotate-ccw" => ("arrow.counterclockwise", '\u{e7a7}'),
        "zoom-in" => ("plus.magnifyingglass", '\u{e8a3}'),
        "zoom-out" => ("minus.magnifyingglass", '\u{e71f}'),
        "maximize" => ("arrow.up.left.and.arrow.down.right", '\u{e740}'),
        "minimize" => ("arrow.down.right.and.arrow.up.left", '\u{e73f}'),
        "monitor" => ("display", '\u{e7f4}'),
        "square-scissors" | "completion-snippet" => ("scissors", '\u{e8c6}'),
        // Completion-kind badges, broken-link status, VCS imagery, and animated
        // artwork retain their bundled shape rather than changing their meaning.
        _ => return None,
    };
    Some(Symbol { macos, windows })
}
