// ABOUTME: GPUI-native Markdown rendering for documentation popups and panels
// ABOUTME: Parses common LSP Markdown into token-aware GPUI elements

use gpui::prelude::FluentBuilder;
use gpui::{
    FontStyle, FontWeight, HighlightStyle, Hsla, IntoElement, ParentElement, RenderOnce,
    SharedString, StrikethroughStyle, Styled, StyledText, UnderlineStyle, Window, div, px,
    relative, rems,
};
use pulldown_cmark::{
    Alignment, CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use crate::tokens::{DesignTokens, with_alpha};

#[derive(Clone, Debug)]
pub struct MarkdownStyle {
    pub body_color: Hsla,
    pub secondary_color: Hsla,
    pub heading_color: Hsla,
    pub link_color: Hsla,
    pub code_background: Hsla,
    pub code_border: Hsla,
    pub quote_border: Hsla,
    pub rule_color: Hsla,
    pub code_font_family: SharedString,
    pub compact: bool,
}

impl MarkdownStyle {
    pub fn from_tokens(tokens: &DesignTokens) -> Self {
        Self {
            body_color: tokens.chrome.text_on_chrome,
            secondary_color: tokens.chrome.text_chrome_secondary,
            heading_color: tokens.chrome.text_on_chrome,
            link_color: tokens.editor.info,
            code_background: with_alpha(tokens.chrome.surface, 0.72),
            code_border: tokens.chrome.border_muted,
            quote_border: tokens.chrome.border_default,
            rule_color: tokens.chrome.border_muted,
            code_font_family: SharedString::from("monospace"),
            compact: false,
        }
    }

    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
}

#[derive(Clone, Debug, IntoElement)]
pub struct MarkdownElement {
    source: SharedString,
    style: MarkdownStyle,
}

pub fn markdown(source: impl Into<SharedString>, style: MarkdownStyle) -> MarkdownElement {
    MarkdownElement {
        source: source.into(),
        style,
    }
}

impl RenderOnce for MarkdownElement {
    fn render(self, _window: &mut Window, _cx: &mut gpui::App) -> impl IntoElement {
        let document = MarkdownDocument::parse(&self.source);
        render_document(document, self.style)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownDocument {
    pub blocks: Vec<MarkdownBlock>,
}

impl MarkdownDocument {
    pub fn parse(source: &str) -> Self {
        MarkdownParser::new(source).parse()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkdownBlock {
    Paragraph(RichText),
    Heading {
        level: u8,
        text: RichText,
    },
    CodeBlock {
        language: Option<String>,
        text: String,
    },
    ListItem {
        ordered: bool,
        index: u64,
        depth: usize,
        checked: Option<bool>,
        text: RichText,
    },
    BlockQuote(RichText),
    Rule,
    Table {
        alignments: Vec<TableAlignment>,
        rows: Vec<Vec<RichText>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAlignment {
    None,
    Left,
    Center,
    Right,
}

impl From<Alignment> for TableAlignment {
    fn from(value: Alignment) -> Self {
        match value {
            Alignment::None => Self::None,
            Alignment::Left => Self::Left,
            Alignment::Center => Self::Center,
            Alignment::Right => Self::Right,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichText {
    spans: Vec<TextSpan>,
}

impl RichText {
    fn push(&mut self, text: impl AsRef<str>, style: InlineStyle) {
        let text = text.as_ref();
        if text.is_empty() {
            return;
        }

        if let Some(last) = self.spans.last_mut()
            && last.style == style
        {
            last.text.push_str(text);
            return;
        }

        self.spans.push(TextSpan {
            text: text.to_string(),
            style,
        });
    }

    fn push_space(&mut self, style: InlineStyle) {
        if !self.is_empty() {
            self.push(" ", style);
        }
    }

    fn is_empty(&self) -> bool {
        self.spans.iter().all(|span| span.text.is_empty())
    }

    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    pub fn spans(&self) -> &[TextSpan] {
        &self.spans
    }

    fn into_highlights(
        self,
        style: &MarkdownStyle,
    ) -> (SharedString, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
        let mut text = String::new();
        let mut highlights = Vec::new();

        for span in self.spans {
            let start = text.len();
            text.push_str(&span.text);
            let end = text.len();

            if let Some(highlight) = span.style.highlight(style) {
                highlights.push((start..end, highlight));
            }
        }

        (SharedString::from(text), highlights)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSpan {
    pub text: String,
    pub style: InlineStyle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub code: bool,
    pub link: bool,
}

impl InlineStyle {
    fn highlight(self, style: &MarkdownStyle) -> Option<HighlightStyle> {
        if self == Self::default() {
            return None;
        }

        Some(HighlightStyle {
            color: self.link.then_some(style.link_color),
            font_weight: self.bold.then_some(FontWeight::BOLD),
            font_style: self.italic.then_some(FontStyle::Italic),
            background_color: self.code.then_some(style.code_background),
            underline: self.link.then_some(UnderlineStyle {
                thickness: px(1.0),
                color: Some(style.link_color),
                wavy: false,
            }),
            strikethrough: self.strikethrough.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(style.body_color),
            }),
            fade_out: None,
        })
    }
}

#[derive(Clone, Debug)]
struct ListContext {
    ordered: bool,
    next_index: u64,
}

struct MarkdownParser {
    events: Vec<Event<'static>>,
    blocks: Vec<MarkdownBlock>,
    current_text: RichText,
    active_style: InlineStyle,
    heading: Option<u8>,
    in_code_block: bool,
    code_block_language: Option<String>,
    code_text: String,
    block_quote_depth: usize,
    list_stack: Vec<ListContext>,
    current_task_marker: Option<bool>,
    link_depth: usize,
    image_depth: usize,
    table_alignments: Vec<TableAlignment>,
    table_rows: Vec<Vec<RichText>>,
    current_table_row: Vec<RichText>,
}

impl MarkdownParser {
    fn new(source: &str) -> Self {
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_GFM;

        Self {
            events: Parser::new_ext(source, options)
                .map(Event::into_static)
                .collect(),
            blocks: Vec::new(),
            current_text: RichText::default(),
            active_style: InlineStyle::default(),
            heading: None,
            in_code_block: false,
            code_block_language: None,
            code_text: String::new(),
            block_quote_depth: 0,
            list_stack: Vec::new(),
            current_task_marker: None,
            link_depth: 0,
            image_depth: 0,
            table_alignments: Vec::new(),
            table_rows: Vec::new(),
            current_table_row: Vec::new(),
        }
    }

    fn parse(mut self) -> MarkdownDocument {
        for event in std::mem::take(&mut self.events) {
            self.handle_event(event);
        }

        MarkdownDocument {
            blocks: self.blocks,
        }
    }

    fn handle_event(&mut self, event: Event<'static>) {
        match event {
            Event::Start(tag) => self.handle_start(tag),
            Event::End(tag) => self.handle_end(tag),
            Event::Text(text) => self.handle_text(&text),
            Event::Code(code) => {
                let mut style = self.active_style;
                style.code = true;
                self.current_text.push(code.as_ref(), style);
            }
            Event::SoftBreak => self.current_text.push_space(self.active_style),
            Event::HardBreak => self.current_text.push("\n", self.active_style),
            Event::Rule => self.blocks.push(MarkdownBlock::Rule),
            Event::TaskListMarker(checked) => {
                self.current_task_marker = Some(checked);
            }
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::FootnoteReference(_) | Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }

    fn handle_start(&mut self, tag: Tag<'static>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.heading = Some(heading_level(level));
            }
            Tag::BlockQuote(_) => {
                self.block_quote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.in_code_block = true;
                self.code_block_language = code_block_language(&kind);
                self.code_text.clear();
            }
            Tag::List(start) => {
                self.list_stack.push(ListContext {
                    ordered: start.is_some(),
                    next_index: start.unwrap_or(1),
                });
            }
            Tag::Item => {
                self.current_text = RichText::default();
                self.current_task_marker = None;
            }
            Tag::Emphasis => self.active_style.italic = true,
            Tag::Strong => self.active_style.bold = true,
            Tag::Strikethrough => self.active_style.strikethrough = true,
            Tag::Link { .. } => {
                self.link_depth += 1;
                self.active_style.link = true;
            }
            Tag::Image { .. } => {
                self.image_depth += 1;
                self.active_style.italic = true;
            }
            Tag::Table(alignments) => {
                self.table_alignments = alignments.into_iter().map(TableAlignment::from).collect();
                self.table_rows.clear();
            }
            Tag::TableHead | Tag::TableRow => {
                self.current_table_row.clear();
            }
            Tag::TableCell => {
                self.current_text = RichText::default();
            }
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_)
            | Tag::HtmlBlock
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn handle_end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if let Some(level) = self.heading.take() {
                    self.flush_heading(level);
                } else if self.block_quote_depth > 0 {
                    self.flush_block_quote();
                } else {
                    self.flush_paragraph();
                }
            }
            TagEnd::Heading(level) => {
                self.heading = None;
                self.flush_heading(heading_level(level));
            }
            TagEnd::BlockQuote(_) => {
                self.block_quote_depth = self.block_quote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => self.flush_code_block(),
            TagEnd::List(_) => {
                self.list_stack.pop();
            }
            TagEnd::Item => self.flush_list_item(),
            TagEnd::Emphasis => self.active_style.italic = false,
            TagEnd::Strong => self.active_style.bold = false,
            TagEnd::Strikethrough => self.active_style.strikethrough = false,
            TagEnd::Link => {
                self.link_depth = self.link_depth.saturating_sub(1);
                if self.link_depth == 0 {
                    self.active_style.link = false;
                }
            }
            TagEnd::Image => {
                self.image_depth = self.image_depth.saturating_sub(1);
                if self.image_depth == 0 {
                    self.active_style.italic = false;
                }
            }
            TagEnd::Table => self.flush_table(),
            TagEnd::TableHead | TagEnd::TableRow => {
                if !self.current_table_row.is_empty() {
                    self.table_rows
                        .push(std::mem::take(&mut self.current_table_row));
                }
            }
            TagEnd::TableCell => {
                self.current_table_row
                    .push(std::mem::take(&mut self.current_text));
            }
            TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_)
            | TagEnd::HtmlBlock
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
        }
    }

    fn handle_text(&mut self, text: &CowStr<'static>) {
        if self.in_code_block {
            self.code_text.push_str(text.as_ref());
        } else {
            self.current_text.push(text.as_ref(), self.active_style);
        }
    }

    fn flush_paragraph(&mut self) {
        if !self.current_text.is_empty() {
            self.blocks.push(MarkdownBlock::Paragraph(std::mem::take(
                &mut self.current_text,
            )));
        }
    }

    fn flush_heading(&mut self, level: u8) {
        if !self.current_text.is_empty() {
            self.blocks.push(MarkdownBlock::Heading {
                level,
                text: std::mem::take(&mut self.current_text),
            });
        }
    }

    fn flush_code_block(&mut self) {
        self.in_code_block = false;
        self.blocks.push(MarkdownBlock::CodeBlock {
            language: self.code_block_language.take(),
            text: std::mem::take(&mut self.code_text),
        });
    }

    fn flush_block_quote(&mut self) {
        if !self.current_text.is_empty() {
            self.blocks.push(MarkdownBlock::BlockQuote(std::mem::take(
                &mut self.current_text,
            )));
        }
    }

    fn flush_list_item(&mut self) {
        if self.current_text.is_empty() {
            return;
        }

        let depth = self.list_stack.len().saturating_sub(1);
        let (ordered, index) = if let Some(context) = self.list_stack.last_mut() {
            let index = context.next_index;
            if context.ordered {
                context.next_index += 1;
            }
            (context.ordered, index)
        } else {
            (false, 1)
        };

        self.blocks.push(MarkdownBlock::ListItem {
            ordered,
            index,
            depth,
            checked: self.current_task_marker.take(),
            text: std::mem::take(&mut self.current_text),
        });
    }

    fn flush_table(&mut self) {
        if !self.table_rows.is_empty() {
            self.blocks.push(MarkdownBlock::Table {
                alignments: std::mem::take(&mut self.table_alignments),
                rows: std::mem::take(&mut self.table_rows),
            });
        }
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn code_block_language(kind: &CodeBlockKind<'_>) -> Option<String> {
    match kind {
        CodeBlockKind::Fenced(info) => info
            .split_whitespace()
            .next()
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned),
        CodeBlockKind::Indented => None,
    }
}

fn render_document(document: MarkdownDocument, style: MarkdownStyle) -> impl IntoElement {
    let gap = if style.compact { px(6.0) } else { px(10.0) };
    let elements: Vec<gpui::AnyElement> = document
        .blocks
        .into_iter()
        .map(|block| match block {
            MarkdownBlock::Paragraph(text) => render_rich_text(text, &style, style.body_color)
                .line_height(relative(1.45))
                .into_any_element(),
            MarkdownBlock::Heading { level, text } => {
                let size = match level {
                    1 => 1.16,
                    2 => 1.08,
                    _ => 1.0,
                };
                render_rich_text(text, &style, style.heading_color)
                    .text_size(rems(size))
                    .font_weight(FontWeight::BOLD)
                    .line_height(relative(1.25))
                    .into_any_element()
            }
            MarkdownBlock::CodeBlock { language, text } => {
                render_code_block(text, language, &style).into_any_element()
            }
            MarkdownBlock::ListItem {
                ordered,
                index,
                depth,
                checked,
                text,
            } => render_list_item(ordered, index, depth, checked, text, &style).into_any_element(),
            MarkdownBlock::BlockQuote(text) => render_block_quote(text, &style).into_any_element(),
            MarkdownBlock::Rule => div()
                .h(px(1.0))
                .w_full()
                .bg(style.rule_color)
                .into_any_element(),
            MarkdownBlock::Table { alignments, rows } => {
                render_table(alignments, rows, &style).into_any_element()
            }
        })
        .collect();

    div().flex().flex_col().gap(gap).children(elements)
}

fn render_rich_text(text: RichText, style: &MarkdownStyle, color: Hsla) -> gpui::Div {
    let (text, highlights) = text.into_highlights(style);

    div()
        .w_full()
        .text_sm()
        .text_color(color)
        .child(StyledText::new(text).with_highlights(highlights))
}

fn render_code_block(
    text: String,
    language: Option<String>,
    style: &MarkdownStyle,
) -> impl IntoElement {
    let content = text.trim_end_matches('\n').to_string();
    let code = div()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(4.0))
        .bg(style.code_background)
        .border_1()
        .border_color(style.code_border)
        .text_sm()
        .line_height(relative(1.45))
        .font_family(style.code_font_family.clone())
        .text_color(style.body_color)
        .overflow_hidden()
        .child(content);

    if let Some(language) = language {
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_xs()
                    .text_color(style.secondary_color)
                    .child(language),
            )
            .child(code)
    } else {
        div().child(code)
    }
}

fn render_list_item(
    ordered: bool,
    index: u64,
    depth: usize,
    checked: Option<bool>,
    text: RichText,
    style: &MarkdownStyle,
) -> impl IntoElement {
    let marker = if let Some(checked) = checked {
        if checked { "[x]" } else { "[ ]" }.to_string()
    } else if ordered {
        format!("{index}.")
    } else {
        "*".to_string()
    };

    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .pl(px((depth as f32) * 16.0))
        .child(
            div()
                .flex_none()
                .w(px(24.0))
                .text_sm()
                .text_color(style.secondary_color)
                .child(marker),
        )
        .child(render_rich_text(text, style, style.body_color).flex_1())
}

fn render_block_quote(text: RichText, style: &MarkdownStyle) -> impl IntoElement {
    div()
        .pl(px(10.0))
        .border_l_2()
        .border_color(style.quote_border)
        .child(render_rich_text(text, style, style.secondary_color).italic())
}

fn render_table(
    alignments: Vec<TableAlignment>,
    rows: Vec<Vec<RichText>>,
    style: &MarkdownStyle,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(style.code_border)
        .rounded(px(4.0))
        .overflow_hidden()
        .children(rows.into_iter().enumerate().map(|(row_index, row)| {
            let header = row_index == 0;
            div()
                .flex()
                .flex_row()
                .when(header, |this| this.bg(style.code_background))
                .when(row_index > 0, |this| {
                    this.border_t_1().border_color(style.code_border)
                })
                .children(row.into_iter().enumerate().map(|(column_index, cell)| {
                    let alignment = alignments
                        .get(column_index)
                        .copied()
                        .unwrap_or(TableAlignment::None);
                    let cell = render_rich_text(cell, style, style.body_color)
                        .px(px(8.0))
                        .py(px(5.0))
                        .when(header, |this| this.font_weight(FontWeight::BOLD))
                        .when(column_index > 0, |this| {
                            this.border_l_1().border_color(style.code_border)
                        });

                    match alignment {
                        TableAlignment::None | TableAlignment::Left => cell,
                        TableAlignment::Center => cell.text_center(),
                        TableAlignment::Right => cell.text_right(),
                    }
                    .flex_1()
                }))
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_lsp_markdown_blocks() {
        let document = MarkdownDocument::parse(
            "# Vec\n\nA **growable** `array`.\n\n```rust\nlet xs = Vec::new();\n```\n\n- fast\n- safe",
        );

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::Heading { level: 1, text } if text.plain_text() == "Vec"
        ));
        assert!(matches!(
            &document.blocks[1],
            MarkdownBlock::Paragraph(text)
                if text.spans().iter().any(|span| span.style.bold)
                    && text.spans().iter().any(|span| span.style.code)
        ));
        assert!(matches!(
            &document.blocks[2],
            MarkdownBlock::CodeBlock { language: Some(language), text }
                if language == "rust" && text.contains("Vec::new")
        ));
        assert!(matches!(
            &document.blocks[3],
            MarkdownBlock::ListItem { ordered: false, index: 1, text, .. }
                if text.plain_text() == "fast"
        ));
    }

    #[test]
    fn parses_links_task_lists_quotes_and_tables() {
        let document = MarkdownDocument::parse(
            "> See [docs](https://example.com)\n\n- [x] done\n- [ ] next\n\n| A | B |\n| :- | -: |\n| left | right |",
        );

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::BlockQuote(text)
                if text.spans().iter().any(|span| span.style.link)
        ));
        assert!(matches!(
            &document.blocks[1],
            MarkdownBlock::ListItem { checked: Some(true), text, .. }
                if text.plain_text() == "done"
        ));
        assert!(matches!(
            &document.blocks[2],
            MarkdownBlock::ListItem { checked: Some(false), text, .. }
                if text.plain_text() == "next"
        ));
        assert!(matches!(
            &document.blocks[3],
            MarkdownBlock::Table { alignments, rows }
                if alignments == &[TableAlignment::Left, TableAlignment::Right]
                    && rows.len() == 2
        ));
    }

    #[test]
    fn ordered_lists_preserve_start_index() {
        let document = MarkdownDocument::parse("3. three\n4. four");

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::ListItem { ordered: true, index: 3, text, .. }
                if text.plain_text() == "three"
        ));
        assert!(matches!(
            &document.blocks[1],
            MarkdownBlock::ListItem { ordered: true, index: 4, text, .. }
                if text.plain_text() == "four"
        ));
    }
}
