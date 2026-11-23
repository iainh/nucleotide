//! The scopes enclosing a document line, read from its tree-sitter syntax tree.
//! The breadcrumb bar shows them for the cursor line and sticky scroll pins
//! their header lines above the first visible line.

use helix_core::{RopeSlice, syntax::Syntax, tree_sitter::Node};
use nucleotide_ui::CompletionItemKind;

/// Most header rows sticky scroll pins at once.
pub(super) const MAX_STICKY_ROWS: usize = 5;

const MAX_LABEL_CHARS: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScopeKind {
    Impl,
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Interface,
    Module,
}

impl ScopeKind {
    /// Classifies declaration nodes by the naming conventions grammars share,
    /// such as `function_item` (Rust), `class_definition` (Python),
    /// `method_declaration` (Go, Java) and `struct_specifier` (C).
    fn from_node_kind(kind: &str) -> Option<Self> {
        let is_declaration = ["_item", "_definition", "_declaration", "_specifier"]
            .iter()
            .any(|suffix| kind.ends_with(suffix))
            // Ruby names its declarations without a suffix.
            || matches!(kind, "class" | "module" | "method" | "singleton_method");
        if !is_declaration {
            return None;
        }
        let has = |word: &str| kind.contains(word);
        Some(if kind.starts_with("impl") {
            Self::Impl
        } else if has("method") || has("constructor") {
            Self::Method
        } else if has("function") {
            Self::Function
        } else if has("class") {
            Self::Class
        } else if has("struct") {
            Self::Struct
        } else if has("enum") {
            Self::Enum
        } else if has("trait") || has("interface") || has("protocol") {
            Self::Interface
        } else if has("mod") || has("namespace") {
            Self::Module
        } else {
            return None;
        })
    }

    pub(super) fn icon_kind(self) -> CompletionItemKind {
        match self {
            Self::Impl | Self::Class => CompletionItemKind::Class,
            Self::Function => CompletionItemKind::Function,
            Self::Method => CompletionItemKind::Method,
            Self::Struct => CompletionItemKind::Struct,
            Self::Enum => CompletionItemKind::Enum,
            Self::Interface => CompletionItemKind::Interface,
            Self::Module => CompletionItemKind::Module,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OutlineScope {
    pub kind: ScopeKind,
    pub label: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// Named multi-line scopes around `line`, outermost first.
pub(super) fn enclosing_scopes(
    syntax: &Syntax,
    text: RopeSlice<'_>,
    line: usize,
) -> Vec<OutlineScope> {
    if text.len_chars() == 0 {
        return Vec::new();
    }
    let line = line.min(text.len_lines() - 1);
    // Start from the line's first non-blank character, so a line holding only
    // a closing brace still belongs to the scope that brace closes.
    let indent = text
        .line(line)
        .chars()
        .take_while(|ch| ch.is_whitespace() && *ch != '\n')
        .count();
    let char_idx = (text.line_to_char(line) + indent).min(text.len_chars() - 1);
    let byte = text.char_to_byte(char_idx) as u32;

    let mut scopes = Vec::new();
    let mut node = syntax.descendant_for_byte_range(byte, byte);
    while let Some(current) = node {
        if let Some(scope) = outline_scope(&current, text) {
            scopes.push(scope);
        }
        node = current.parent();
    }
    scopes.reverse();
    scopes
}

fn outline_scope(node: &Node<'_>, text: RopeSlice<'_>) -> Option<OutlineScope> {
    let kind = ScopeKind::from_node_kind(node.kind())?;
    let range = node.byte_range();
    let start_line = text.byte_to_line(range.start as usize);
    let end_line = text.byte_to_line(
        (range.end as usize)
            .saturating_sub(1)
            .max(range.start as usize),
    );
    if end_line <= start_line {
        return None;
    }
    let header_end = text.line_to_byte(start_line + 1).min(range.end as usize);
    let header = String::from(text.byte_slice(range.start as usize..header_end));

    let label = if kind == ScopeKind::Impl {
        impl_label(&header)
    } else {
        let name = scope_name(node)?;
        let name_range = name.byte_range();
        let name_text =
            String::from(text.byte_slice(name_range.start as usize..name_range.end as usize));
        let prefix_len = (name_range.start - range.start) as usize;
        declaration_label(header.get(..prefix_len).unwrap_or(""), &name_text)
    };
    (!label.is_empty()).then(|| OutlineScope {
        kind,
        label: truncate_label(label),
        start_line,
        end_line,
    })
}

/// The identifier a declaration names, looking through C-style declarators.
fn scope_name<'tree>(node: &Node<'tree>) -> Option<Node<'tree>> {
    fn find<'tree>(node: &Node<'tree>, depth: usize) -> Option<Node<'tree>> {
        node.children().filter(Node::is_named).find_map(|child| {
            let kind = child.kind();
            if kind.ends_with("identifier") || matches!(kind, "name" | "constant") {
                Some(child)
            } else if depth > 0 && kind.ends_with("declarator") {
                find(&child, depth - 1)
            } else {
                None
            }
        })
    }
    find(node, 3)
}

/// `fn render`, `class Workspace`, or the bare name when no keyword leads it,
/// such as a Go method whose receiver comes before the name.
fn declaration_label(prefix: &str, name: &str) -> String {
    let keyword = prefix
        .split_whitespace()
        .next_back()
        .filter(|word| word.chars().all(|ch| ch.is_ascii_alphabetic()));
    match keyword {
        Some(keyword) => format!("{keyword} {name}"),
        None => name.to_string(),
    }
}

/// `impl Render for Workspace` from the header line of an impl block.
fn impl_label(header: &str) -> String {
    let header = header.split('{').next().unwrap_or(header);
    let header = header.split(" where").next().unwrap_or(header);
    let words: Vec<&str> = header.split_whitespace().collect();
    let start = words
        .iter()
        .position(|word| word.starts_with("impl"))
        .unwrap_or(0);
    words[start..].join(" ")
}

fn truncate_label(label: String) -> String {
    match label.char_indices().nth(MAX_LABEL_CHARS) {
        Some((index, _)) => format!("{}…", &label[..index]),
        None => label,
    }
}

/// Scopes whose header lines sticky scroll pins when `top_line` is the first
/// visible line, outermost first. Pinned rows cover the lines below the top
/// too, so a scope that starts under them is pinned as well, and a scope
/// whose remaining lines would all be covered is not.
pub(super) fn sticky_scopes(
    top_line: usize,
    max_rows: usize,
    scopes_at: impl Fn(usize) -> Vec<OutlineScope>,
) -> Vec<OutlineScope> {
    let mut rows = Vec::new();
    loop {
        let first_uncovered = top_line + rows.len();
        let mut next: Vec<_> = scopes_at(first_uncovered)
            .into_iter()
            .filter(|scope| scope.start_line < first_uncovered)
            .take(max_rows)
            .collect();
        // Inner scopes end first, so only the innermost rows can be fully covered.
        while next
            .last()
            .is_some_and(|scope| scope.end_line < top_line + next.len())
        {
            next.pop();
        }
        if next.len() <= rows.len() {
            return rows;
        }
        rows = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(start_line: usize, end_line: usize) -> OutlineScope {
        OutlineScope {
            kind: ScopeKind::Function,
            label: format!("fn at_{start_line}"),
            start_line,
            end_line,
        }
    }

    #[test]
    fn classifies_declarations_across_grammars() {
        for (kind, expected) in [
            ("impl_item", Some(ScopeKind::Impl)),
            ("function_item", Some(ScopeKind::Function)),
            ("function_definition", Some(ScopeKind::Function)),
            ("method_declaration", Some(ScopeKind::Method)),
            ("class_declaration", Some(ScopeKind::Class)),
            ("struct_specifier", Some(ScopeKind::Struct)),
            ("trait_item", Some(ScopeKind::Interface)),
            ("mod_item", Some(ScopeKind::Module)),
            ("method", Some(ScopeKind::Method)),
            ("call_expression", None),
            ("function_call", None),
            ("field_declaration", None),
            ("block", None),
        ] {
            assert_eq!(ScopeKind::from_node_kind(kind), expected, "{kind}");
        }
    }

    #[test]
    fn labels_keep_the_declaring_keyword_only() {
        assert_eq!(
            declaration_label("pub(crate) async fn ", "render"),
            "fn render"
        );
        assert_eq!(declaration_label("class ", "Workspace"), "class Workspace");
        assert_eq!(
            declaration_label("func (w *Workspace) ", "Render"),
            "Render"
        );
        assert_eq!(declaration_label("", "render"), "render");
        assert_eq!(
            impl_label("impl<T: Clone> Render for Workspace<T> where T: Send {\n"),
            "impl<T: Clone> Render for Workspace<T>"
        );
        assert_eq!(
            impl_label("unsafe impl Send for Core {}"),
            "impl Send for Core"
        );
    }

    #[test]
    fn sticky_scopes_pin_only_headers_above_the_visible_lines() {
        // impl 0..=100 holds fn 10..=20 and fn 30..=40.
        let scopes_at = |line: usize| {
            [scope(0, 100), scope(10, 20), scope(30, 40)]
                .into_iter()
                .filter(|scope| scope.start_line <= line && line <= scope.end_line)
                .collect()
        };

        assert!(sticky_scopes(0, 5, scopes_at).is_empty());
        // The impl row covers line 9; the fn header on line 10 is still visible.
        assert_eq!(sticky_scopes(9, 5, scopes_at), vec![scope(0, 100)]);
        // The impl row covers line 10, so the fn header under it is pinned too.
        assert_eq!(
            sticky_scopes(10, 5, scopes_at),
            vec![scope(0, 100), scope(10, 20)]
        );
        assert_eq!(
            sticky_scopes(15, 5, scopes_at),
            vec![scope(0, 100), scope(10, 20)]
        );
        // Line 20 closes the fn; pinning the fn would cover it.
        assert_eq!(sticky_scopes(19, 5, scopes_at), vec![scope(0, 100)]);
        assert_eq!(
            sticky_scopes(18, 5, scopes_at),
            vec![scope(0, 100), scope(10, 20)]
        );
        assert_eq!(sticky_scopes(22, 5, scopes_at), vec![scope(0, 100)]);
        assert_eq!(sticky_scopes(15, 1, scopes_at), vec![scope(0, 100)]);
    }

    #[test]
    fn enclosing_scopes_follow_the_rust_syntax_tree() {
        let source = "\
mod outer {
    impl Render for Workspace {
        pub fn render(&mut self) {
            let x = 1;

        }
    }
}
";
        let loader = helix_core::syntax::Loader::new(
            helix_loader::config::default_lang_config()
                .try_into()
                .unwrap(),
        )
        .unwrap();
        let rope = helix_core::Rope::from(source);
        let Some(language) = loader.language_for_name("rust") else {
            return;
        };
        let Ok(syntax) = Syntax::new(rope.slice(..), language, &loader) else {
            // Grammars are built separately (`nucl-grammar fetch build`).
            eprintln!("skipping: the Rust tree-sitter grammar is not built");
            return;
        };
        let labels = |line| {
            enclosing_scopes(&syntax, rope.slice(..), line)
                .into_iter()
                .map(|scope| (scope.label, scope.start_line, scope.end_line))
                .collect::<Vec<_>>()
        };

        let all = vec![
            ("mod outer".to_string(), 0, 7),
            ("impl Render for Workspace".to_string(), 1, 6),
            ("fn render".to_string(), 2, 5),
        ];
        assert_eq!(labels(3), all);
        // Blank and closing-brace lines belong to the scope around them.
        assert_eq!(labels(4), all);
        assert_eq!(labels(5), all);
        assert_eq!(labels(6), all[..2].to_vec());
        assert_eq!(labels(0), all[..1].to_vec());
    }
}
