// ABOUTME: GPUI-native Markdown rendering for documentation popups and panels
// ABOUTME: Parses common LSP Markdown into token-aware GPUI elements

use gpui::prelude::{FluentBuilder, StyledImage};
use gpui::{
    AppContext, ElementId, FontStyle, FontWeight, HighlightStyle, Hsla, InteractiveElement,
    InteractiveText, IntoElement, MouseButton, ParentElement, Pixels, Render, RenderOnce,
    SharedString, StatefulInteractiveElement, StrikethroughStyle, Styled, StyledText,
    UnderlineStyle, Window, div, img, px, relative, rems,
};
use helix_core::{
    RopeSlice, Syntax,
    syntax::{self, HighlightEvent},
};
use helix_view::graphics::{
    Modifier as HelixModifier, Style as HelixStyle, UnderlineStyle as HelixUnderlineStyle,
};
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, CowStr, Event, HeadingLevel, LinkType, Options,
    Parser, Tag, TagEnd,
};
use std::{
    borrow::Cow,
    ops::Range,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use crate::theme_utils::color_to_hsla;
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
    pub preview: bool,
    pub body_font_size: Pixels,
    pub heading_font_sizes: [Pixels; 6],
    pub heading_border_color: Option<Hsla>,
    pub code_overflow_x_scroll: bool,
    pub table_header_background: Hsla,
    pub table_alternate_background: Hsla,
    pub alert_note_color: Hsla,
    pub alert_tip_color: Hsla,
    pub alert_important_color: Hsla,
    pub alert_warning_color: Hsla,
    pub alert_caution_color: Hsla,
}

impl MarkdownStyle {
    pub fn from_tokens(tokens: &DesignTokens) -> Self {
        let body_font_size = tokens.sizes.text_sm;
        Self {
            body_color: tokens.chrome.text_on_chrome,
            secondary_color: tokens.chrome.text_chrome_secondary,
            heading_color: tokens.chrome.text_on_chrome,
            link_color: tokens.editor.info,
            code_background: tokens.editor.background,
            code_border: tokens.chrome.border_muted,
            quote_border: tokens.chrome.border_default,
            rule_color: tokens.chrome.border_muted,
            code_font_family: SharedString::from("monospace"),
            compact: false,
            preview: false,
            body_font_size,
            heading_font_sizes: scaled_heading_sizes(body_font_size),
            heading_border_color: None,
            code_overflow_x_scroll: false,
            table_header_background: tokens.editor.background,
            table_alternate_background: with_alpha(tokens.chrome.surface_hover, 0.28),
            alert_note_color: tokens.editor.info,
            alert_tip_color: tokens.editor.success,
            alert_important_color: tokens.editor.info,
            alert_warning_color: tokens.editor.warning,
            alert_caution_color: tokens.editor.error,
        }
    }

    pub fn preview_from_tokens(tokens: &DesignTokens) -> Self {
        let body_font_size = tokens.sizes.text_base;
        let mut style = Self::from_tokens(tokens);
        style.preview = true;
        style.body_font_size = body_font_size;
        style.heading_font_sizes = scaled_heading_sizes(body_font_size);
        style.heading_border_color = Some(tokens.chrome.border_muted);
        style.code_overflow_x_scroll = true;
        style.table_header_background = tokens.chrome.surface_hover;
        style.table_alternate_background = with_alpha(tokens.chrome.surface_hover, 0.32);
        style
    }

    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
}

fn scaled_heading_sizes(body_font_size: Pixels) -> [Pixels; 6] {
    let base = f32::from(body_font_size);
    [
        px(base * 1.85),
        px(base * 1.55),
        px(base * 1.30),
        px(base * 1.15),
        body_font_size,
        px(base * 0.90),
    ]
}

#[derive(Clone)]
pub struct MarkdownSyntaxLoader {
    loader: Arc<syntax::Loader>,
}

impl MarkdownSyntaxLoader {
    pub fn new(loader: Arc<syntax::Loader>) -> Self {
        Self { loader }
    }

    fn loader(&self) -> &syntax::Loader {
        &self.loader
    }
}

impl gpui::Global for MarkdownSyntaxLoader {}

#[derive(Clone, Debug, IntoElement)]
pub struct MarkdownElement {
    source: SharedString,
    style: MarkdownStyle,
    parse_mode: MarkdownParseMode,
}

pub fn markdown(source: impl Into<SharedString>, style: MarkdownStyle) -> MarkdownElement {
    MarkdownElement {
        source: source.into(),
        style,
        parse_mode: MarkdownParseMode::CommonMark,
    }
}

pub fn markdown_extended(source: impl Into<SharedString>, style: MarkdownStyle) -> MarkdownElement {
    MarkdownElement {
        source: source.into(),
        style,
        parse_mode: MarkdownParseMode::Extended,
    }
}

impl RenderOnce for MarkdownElement {
    fn render(self, _window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let document = MarkdownDocument::parse_with_mode(&self.source, self.parse_mode);
        let helix_theme = cx
            .try_global::<crate::theme_manager::ThemeManager>()
            .map(|theme_manager| theme_manager.helix_theme());
        let syntax_loader = cx
            .try_global::<MarkdownSyntaxLoader>()
            .map(MarkdownSyntaxLoader::loader);

        render_document(document, self.style, helix_theme, syntax_loader)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownDocument {
    pub blocks: Vec<MarkdownBlock>,
}

impl MarkdownDocument {
    pub fn parse(source: &str) -> Self {
        Self::parse_with_mode(source, MarkdownParseMode::CommonMark)
    }

    pub fn parse_extended(source: &str) -> Self {
        Self::parse_with_mode(source, MarkdownParseMode::Extended)
    }

    pub fn parse_with_mode(source: &str, mode: MarkdownParseMode) -> Self {
        MarkdownParser::new(source, mode).parse()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownParseMode {
    CommonMark,
    Extended,
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
    HtmlBlock {
        text: String,
    },
    Image {
        url: SharedString,
        alt: RichText,
        title: Option<SharedString>,
        link_url: Option<SharedString>,
        link_title: Option<SharedString>,
    },
    ListItem {
        ordered: bool,
        index: u64,
        depth: usize,
        checked: Option<bool>,
        text: RichText,
        children: Vec<MarkdownBlock>,
    },
    BlockQuote {
        kind: Option<MarkdownAlertKind>,
        blocks: Vec<MarkdownBlock>,
    },
    Rule,
    Table {
        alignments: Vec<TableAlignment>,
        rows: Vec<Vec<RichText>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownAlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl MarkdownAlertKind {
    fn from_block_quote_kind(kind: BlockQuoteKind) -> Self {
        match kind {
            BlockQuoteKind::Note => Self::Note,
            BlockQuoteKind::Tip => Self::Tip,
            BlockQuoteKind::Important => Self::Important,
            BlockQuoteKind::Warning => Self::Warning,
            BlockQuoteKind::Caution => Self::Caution,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }

    fn color(self, style: &MarkdownStyle) -> Hsla {
        match self {
            Self::Note => style.alert_note_color,
            Self::Tip => style.alert_tip_color,
            Self::Important => style.alert_important_color,
            Self::Warning => style.alert_warning_color,
            Self::Caution => style.alert_caution_color,
        }
    }
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
    inline_images: Vec<InlineImage>,
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

    fn text_len(&self) -> usize {
        self.spans.iter().map(|span| span.text.len()).sum()
    }

    fn push_inline_image(&mut self, image: InlineImage) {
        self.inline_images.push(image);
    }

    fn inline_images(&self) -> &[InlineImage] {
        &self.inline_images
    }

    fn slice(&self, range: Range<usize>) -> Self {
        let mut text = Self::default();
        let mut cursor = 0;

        for span in &self.spans {
            let span_start = cursor;
            let span_end = span_start + span.text.len();
            cursor = span_end;

            let start = range.start.max(span_start);
            let end = range.end.min(span_end);
            if start >= end {
                continue;
            }

            let local_start = next_char_boundary(&span.text, start - span_start);
            let local_end = previous_char_boundary(&span.text, end - span_start);
            if local_start >= local_end {
                continue;
            }

            text.push(&span.text[local_start..local_end], span.style.clone());
        }

        text
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

    fn into_render_parts(self, style: &MarkdownStyle) -> RichTextRenderParts {
        let mut text = String::new();
        let mut highlights = Vec::new();
        let mut links = Vec::new();

        for span in self.spans {
            let start = text.len();
            text.push_str(&span.text);
            let end = text.len();

            if let Some(highlight) = span.style.highlight(style) {
                highlights.push((start..end, highlight));
            }

            if let Some(url) = span.style.link_url
                && start < end
            {
                links.push(LinkRange {
                    range: start..end,
                    url,
                    title: span.style.link_title,
                });
            }
        }

        RichTextRenderParts {
            text: SharedString::from(text),
            highlights,
            links,
        }
    }
}

fn previous_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSpan {
    pub text: String,
    pub style: InlineStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InlineImage {
    range: Range<usize>,
    url: SharedString,
    alt: RichText,
    title: Option<SharedString>,
    link_url: Option<SharedString>,
    link_title: Option<SharedString>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LinkRange {
    range: Range<usize>,
    url: SharedString,
    title: Option<SharedString>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RichTextRenderParts {
    text: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<LinkRange>,
}

#[derive(Clone)]
struct MarkdownTooltip {
    text: SharedString,
    style: MarkdownStyle,
}

impl Render for MarkdownTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(420.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(self.style.code_border)
            .bg(self.style.code_background)
            .text_size(self.style.body_font_size)
            .text_color(self.style.body_color)
            .child(self.text.clone())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub code: bool,
    pub link: bool,
    pub link_url: Option<SharedString>,
    pub link_title: Option<SharedString>,
}

impl InlineStyle {
    fn highlight(&self, style: &MarkdownStyle) -> Option<HighlightStyle> {
        if self == &Self::default() {
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

#[derive(Clone, Debug)]
struct ImageContext {
    dest_url: SharedString,
    title: Option<SharedString>,
    start_text_len: usize,
    nesting_depth: usize,
    link_url: Option<SharedString>,
    link_title: Option<SharedString>,
}

#[derive(Clone, Debug)]
struct LinkContext {
    url: SharedString,
    title: Option<SharedString>,
}

#[derive(Clone, Debug)]
struct ParsedImage {
    dest_url: SharedString,
    title: Option<SharedString>,
    start_text_len: usize,
    end_text_len: usize,
    fallback_inserted: bool,
    nesting_depth: usize,
    link_url: Option<SharedString>,
    link_title: Option<SharedString>,
}

#[derive(Clone, Debug)]
struct ListItemBuilder {
    ordered: bool,
    index: u64,
    depth: usize,
    checked: Option<bool>,
    text: RichText,
    children: Vec<MarkdownBlock>,
}

impl ListItemBuilder {
    fn into_block(self) -> MarkdownBlock {
        MarkdownBlock::ListItem {
            ordered: self.ordered,
            index: self.index,
            depth: self.depth,
            checked: self.checked,
            text: self.text,
            children: self.children,
        }
    }
}

#[derive(Clone, Debug)]
struct BlockQuoteBuilder {
    kind: Option<MarkdownAlertKind>,
    blocks: Vec<MarkdownBlock>,
}

#[derive(Clone, Debug)]
enum MarkdownContainer {
    ListItem(ListItemBuilder),
    BlockQuote(BlockQuoteBuilder),
}

impl MarkdownContainer {
    fn children_mut(&mut self) -> &mut Vec<MarkdownBlock> {
        match self {
            Self::ListItem(item) => &mut item.children,
            Self::BlockQuote(quote) => &mut quote.blocks,
        }
    }
}

struct MarkdownParser {
    events: Vec<Event<'static>>,
    blocks: Vec<MarkdownBlock>,
    containers: Vec<MarkdownContainer>,
    current_text: RichText,
    active_style: InlineStyle,
    emphasis_depth: usize,
    strong_depth: usize,
    strikethrough_depth: usize,
    code_depth: usize,
    html_block_emphasis_depth: usize,
    html_block_strong_depth: usize,
    html_block_strikethrough_depth: usize,
    html_block_code_depth: usize,
    heading: Option<u8>,
    in_code_block: bool,
    code_block_language: Option<String>,
    code_text: String,
    in_html_block: bool,
    html_text: String,
    list_stack: Vec<ListContext>,
    current_task_marker: Option<bool>,
    link_stack: Vec<LinkContext>,
    html_link_stack: Vec<Option<LinkContext>>,
    html_block_link_stack: Vec<Option<LinkContext>>,
    image_depth: usize,
    table_alignments: Vec<TableAlignment>,
    table_rows: Vec<Vec<RichText>>,
    current_table_row: Vec<RichText>,
    in_table: bool,
    image_stack: Vec<ImageContext>,
    current_inline_images: Vec<ParsedImage>,
    current_block_has_inline_content: bool,
}

impl MarkdownParser {
    fn new(source: &str, mode: MarkdownParseMode) -> Self {
        let source = normalize_commonmark_source(source);
        let options = match mode {
            MarkdownParseMode::CommonMark => Options::empty(),
            MarkdownParseMode::Extended => {
                Options::ENABLE_TABLES
                    | Options::ENABLE_STRIKETHROUGH
                    | Options::ENABLE_TASKLISTS
                    | Options::ENABLE_GFM
            }
        };

        Self {
            events: Parser::new_ext(source.as_ref(), options)
                .map(Event::into_static)
                .collect(),
            blocks: Vec::new(),
            containers: Vec::new(),
            current_text: RichText::default(),
            active_style: InlineStyle::default(),
            emphasis_depth: 0,
            strong_depth: 0,
            strikethrough_depth: 0,
            code_depth: 0,
            html_block_emphasis_depth: 0,
            html_block_strong_depth: 0,
            html_block_strikethrough_depth: 0,
            html_block_code_depth: 0,
            heading: None,
            in_code_block: false,
            code_block_language: None,
            code_text: String::new(),
            in_html_block: false,
            html_text: String::new(),
            list_stack: Vec::new(),
            current_task_marker: None,
            link_stack: Vec::new(),
            html_link_stack: Vec::new(),
            html_block_link_stack: Vec::new(),
            image_depth: 0,
            table_alignments: Vec::new(),
            table_rows: Vec::new(),
            current_table_row: Vec::new(),
            in_table: false,
            image_stack: Vec::new(),
            current_inline_images: Vec::new(),
            current_block_has_inline_content: false,
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
                self.current_block_has_inline_content = true;
                let mut style = self.active_style.clone();
                style.code = true;
                self.current_text.push(code.as_ref(), style);
            }
            Event::SoftBreak => self.current_text.push_space(self.active_style.clone()),
            Event::HardBreak => {
                self.current_block_has_inline_content = true;
                self.current_text.push("\n", self.active_style.clone());
            }
            Event::Rule => {
                self.flush_paragraph();
                self.push_block(MarkdownBlock::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(MarkdownContainer::ListItem(item)) = self.containers.last_mut() {
                    item.checked = Some(checked);
                } else {
                    self.current_task_marker = Some(checked);
                }
            }
            Event::Html(html) => {
                if self.in_html_block {
                    self.html_text.push_str(html.as_ref());
                } else {
                    self.handle_inline_html(html.as_ref());
                }
            }
            Event::InlineHtml(html) => {
                self.handle_inline_html(html.as_ref());
            }
            Event::FootnoteReference(_) | Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }

    fn handle_inline_html(&mut self, html: &str) {
        self.current_block_has_inline_content = true;
        if inline_html_is_line_break(html) {
            self.current_text.push("\n", self.active_style.clone());
        } else if let Some(tag) = inline_html_link_tag(html) {
            match tag {
                InlineHtmlLinkTag::Open(link) => {
                    self.html_link_stack.push(link);
                }
                InlineHtmlLinkTag::Close => {
                    self.html_link_stack.pop();
                }
            }
            self.refresh_link_style();
        } else if let Some((tag, closing)) = inline_html_style_tag(html) {
            if closing {
                self.decrement_inline_html_style(tag);
            } else {
                self.increment_inline_html_style(tag);
            }
            self.refresh_inline_style_flags();
        }
    }

    fn increment_inline_html_style(&mut self, tag: InlineHtmlStyleTag) {
        match tag {
            InlineHtmlStyleTag::Emphasis => self.emphasis_depth += 1,
            InlineHtmlStyleTag::Strong => self.strong_depth += 1,
            InlineHtmlStyleTag::Strikethrough => self.strikethrough_depth += 1,
            InlineHtmlStyleTag::Code => self.code_depth += 1,
        }
    }

    fn decrement_inline_html_style(&mut self, tag: InlineHtmlStyleTag) {
        match tag {
            InlineHtmlStyleTag::Emphasis => {
                self.emphasis_depth = self.emphasis_depth.saturating_sub(1);
            }
            InlineHtmlStyleTag::Strong => {
                self.strong_depth = self.strong_depth.saturating_sub(1);
            }
            InlineHtmlStyleTag::Strikethrough => {
                self.strikethrough_depth = self.strikethrough_depth.saturating_sub(1);
            }
            InlineHtmlStyleTag::Code => {
                self.code_depth = self.code_depth.saturating_sub(1);
            }
        }
    }

    fn handle_start(&mut self, tag: Tag<'static>) {
        match tag {
            Tag::Paragraph => {
                self.current_inline_images.clear();
                self.current_block_has_inline_content = false;
            }
            Tag::Heading { level, .. } => {
                self.current_inline_images.clear();
                self.heading = Some(heading_level(level));
                self.current_block_has_inline_content = false;
            }
            Tag::BlockQuote(kind) => {
                self.flush_current_text_into_open_list_item();
                self.containers
                    .push(MarkdownContainer::BlockQuote(BlockQuoteBuilder {
                        kind: kind.map(MarkdownAlertKind::from_block_quote_kind),
                        blocks: Vec::new(),
                    }));
            }
            Tag::CodeBlock(kind) => {
                self.flush_current_text_into_open_list_item();
                self.in_code_block = true;
                self.code_block_language = code_block_language(&kind);
                self.code_text.clear();
            }
            Tag::List(start) => {
                self.flush_current_text_into_open_list_item();
                self.list_stack.push(ListContext {
                    ordered: start.is_some(),
                    next_index: start.unwrap_or(1),
                });
            }
            Tag::Item => {
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

                self.containers
                    .push(MarkdownContainer::ListItem(ListItemBuilder {
                        ordered,
                        index,
                        depth,
                        checked: None,
                        text: RichText::default(),
                        children: Vec::new(),
                    }));
                self.current_text = RichText::default();
                self.current_inline_images.clear();
                self.current_task_marker = None;
            }
            Tag::Emphasis => {
                self.emphasis_depth += 1;
                self.refresh_inline_style_flags();
            }
            Tag::Strong => {
                self.strong_depth += 1;
                self.refresh_inline_style_flags();
            }
            Tag::Strikethrough => {
                self.strikethrough_depth += 1;
                self.refresh_inline_style_flags();
            }
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => {
                self.current_block_has_inline_content = true;
                let context = LinkContext {
                    url: link_url(link_type, &dest_url),
                    title: nonempty_shared_string(&title),
                };
                self.link_stack.push(context);
                self.active_style.link = true;
                self.refresh_link_style();
            }
            Tag::Image {
                dest_url, title, ..
            } => {
                self.current_block_has_inline_content = true;
                let nesting_depth = self.image_depth;
                let (link_url, link_title) = self
                    .active_link_context()
                    .map(|link| (Some(link.url.clone()), link.title.clone()))
                    .unwrap_or((None, None));
                self.image_depth += 1;
                self.image_stack.push(ImageContext {
                    dest_url: commonmark_url(&dest_url),
                    title: nonempty_shared_string(&title),
                    start_text_len: self.current_text.text_len(),
                    nesting_depth,
                    link_url,
                    link_title,
                });
                self.refresh_inline_style_flags();
                self.refresh_link_style();
            }
            Tag::Table(alignments) => {
                self.flush_current_text_into_open_list_item();
                self.table_alignments = alignments.into_iter().map(TableAlignment::from).collect();
                self.table_rows.clear();
                self.in_table = true;
            }
            Tag::TableHead | Tag::TableRow => {
                self.current_table_row.clear();
            }
            Tag::TableCell => {
                self.current_text = RichText::default();
                self.current_inline_images.clear();
                self.current_block_has_inline_content = false;
            }
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_)
            | Tag::Superscript
            | Tag::Subscript => {}
            Tag::HtmlBlock => {
                self.flush_current_text_into_open_list_item();
                self.in_html_block = true;
                self.html_text.clear();
            }
        }
    }

    fn handle_end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if self.in_table {
                    return;
                }

                if let Some(level) = self.heading.take() {
                    self.flush_heading(level);
                } else {
                    self.flush_paragraph();
                }
            }
            TagEnd::Heading(level) => {
                self.heading = None;
                self.flush_heading(heading_level(level));
            }
            TagEnd::BlockQuote(_) => {
                self.flush_paragraph();
                let Some(MarkdownContainer::BlockQuote(quote)) = self.containers.pop() else {
                    return;
                };
                self.push_block(MarkdownBlock::BlockQuote {
                    kind: quote.kind,
                    blocks: quote.blocks,
                });
            }
            TagEnd::CodeBlock => self.flush_code_block(),
            TagEnd::List(_) => {
                self.list_stack.pop();
            }
            TagEnd::Item => self.flush_list_item(),
            TagEnd::Emphasis => {
                self.emphasis_depth = self.emphasis_depth.saturating_sub(1);
                self.refresh_inline_style_flags();
            }
            TagEnd::Strong => {
                self.strong_depth = self.strong_depth.saturating_sub(1);
                self.refresh_inline_style_flags();
            }
            TagEnd::Strikethrough => {
                self.strikethrough_depth = self.strikethrough_depth.saturating_sub(1);
                self.refresh_inline_style_flags();
            }
            TagEnd::Link => {
                self.link_stack.pop();
                self.refresh_link_style();
            }
            TagEnd::Image => {
                if let Some(context) = self.image_stack.pop() {
                    let fallback_inserted = self.current_text.text_len() == context.start_text_len;
                    let dest_url = context.dest_url.clone();
                    if fallback_inserted {
                        self.current_text
                            .push(dest_url.as_ref(), self.active_style.clone());
                    }
                    let end_text_len = self.current_text.text_len();
                    let alt = if fallback_inserted {
                        RichText::default()
                    } else {
                        rich_text_from_plain(
                            self.current_text
                                .slice(context.start_text_len..end_text_len)
                                .plain_text(),
                        )
                    };
                    if context.nesting_depth == 0 {
                        self.current_text.push_inline_image(InlineImage {
                            range: context.start_text_len..end_text_len,
                            url: dest_url.clone(),
                            alt,
                            title: context.title.clone(),
                            link_url: context.link_url.clone(),
                            link_title: context.link_title.clone(),
                        });
                    }
                    self.current_inline_images.push(ParsedImage {
                        dest_url,
                        title: context.title,
                        start_text_len: context.start_text_len,
                        end_text_len,
                        fallback_inserted,
                        nesting_depth: context.nesting_depth,
                        link_url: context.link_url,
                        link_title: context.link_title,
                    });
                }
                self.image_depth = self.image_depth.saturating_sub(1);
                self.refresh_inline_style_flags();
                self.refresh_link_style();
            }
            TagEnd::Table => {
                self.in_table = false;
                self.flush_table();
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if !self.current_table_row.is_empty() {
                    self.table_rows
                        .push(std::mem::take(&mut self.current_table_row));
                }
            }
            TagEnd::TableCell => {
                self.current_table_row
                    .push(std::mem::take(&mut self.current_text));
                self.current_inline_images.clear();
                self.current_block_has_inline_content = false;
                self.reset_inline_style_state();
            }
            TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_)
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
            TagEnd::HtmlBlock => self.flush_html_block(),
        }
    }

    fn handle_text(&mut self, text: &CowStr<'static>) {
        if self.in_code_block {
            self.code_text.push_str(text.as_ref());
        } else {
            self.current_block_has_inline_content = true;
            self.current_text
                .push(text.as_ref(), self.active_style.clone());
        }
    }

    fn flush_paragraph(&mut self) {
        self.flush_current_text_as_paragraph();
    }

    fn flush_heading(&mut self, level: u8) {
        let text = std::mem::take(&mut self.current_text);
        self.current_inline_images.clear();
        self.current_block_has_inline_content = false;
        self.push_block(MarkdownBlock::Heading { level, text });
        self.reset_inline_style_state();
    }

    fn flush_code_block(&mut self) {
        self.in_code_block = false;
        let language = self.code_block_language.take();
        let text = std::mem::take(&mut self.code_text);
        self.push_block(MarkdownBlock::CodeBlock { language, text });
    }

    fn flush_list_item(&mut self) {
        self.flush_current_text_into_open_list_item();
        let Some(MarkdownContainer::ListItem(mut item)) = self.containers.pop() else {
            return;
        };

        if item.checked.is_none() {
            item.checked = self.current_task_marker.take();
        }

        self.push_block(item.into_block());
    }

    fn flush_table(&mut self) {
        if !self.table_rows.is_empty() {
            let alignments = std::mem::take(&mut self.table_alignments);
            let rows = std::mem::take(&mut self.table_rows);
            self.push_block(MarkdownBlock::Table { alignments, rows });
        }
    }

    fn flush_html_block(&mut self) {
        self.in_html_block = false;
        if !self.html_text.is_empty() {
            let text = std::mem::take(&mut self.html_text);
            self.update_html_block_context(&text);
            self.push_block(MarkdownBlock::HtmlBlock { text });
        }
    }

    fn flush_current_text_into_open_list_item(&mut self) {
        if self.current_text.is_empty() && !self.current_block_has_inline_content {
            self.current_inline_images.clear();
            return;
        }

        if matches!(self.containers.last(), Some(MarkdownContainer::ListItem(_))) {
            self.flush_current_text_as_paragraph();
        }
    }

    fn flush_current_text_as_paragraph(&mut self) {
        let text_is_empty = self.current_text.is_empty();
        if text_is_empty && !self.current_block_has_inline_content {
            self.current_inline_images.clear();
            return;
        }

        let text = std::mem::take(&mut self.current_text);
        self.current_block_has_inline_content = false;
        if let Some(image) = self.take_standalone_image_block(&text) {
            self.push_block(image);
            self.reset_inline_style_state();
            return;
        }

        self.current_inline_images.clear();
        if let Some(MarkdownContainer::ListItem(item)) = self.containers.last_mut()
            && item.text.is_empty()
            && item.children.is_empty()
            && !text_is_empty
        {
            item.text = text;
            self.reset_inline_style_state();
            return;
        }

        self.push_block(MarkdownBlock::Paragraph(text));
        self.reset_inline_style_state();
    }

    fn take_standalone_image_block(&mut self, text: &RichText) -> Option<MarkdownBlock> {
        let image = self
            .current_inline_images
            .iter()
            .find(|image| {
                image.nesting_depth == 0
                    && image.start_text_len == 0
                    && image.end_text_len == text.text_len()
            })?
            .clone();
        self.current_inline_images.clear();

        let alt = if image.fallback_inserted {
            RichText::default()
        } else {
            rich_text_from_plain(text.plain_text())
        };

        Some(MarkdownBlock::Image {
            url: image.dest_url,
            alt,
            title: image.title,
            link_url: image.link_url,
            link_title: image.link_title,
        })
    }

    fn push_block(&mut self, block: MarkdownBlock) {
        if let Some(container) = self.containers.last_mut() {
            container.children_mut().push(block);
        } else {
            self.blocks.push(block);
        }
    }

    fn refresh_inline_style_flags(&mut self) {
        self.active_style.italic = self.emphasis_depth + self.html_block_emphasis_depth > 0;
        self.active_style.bold = self.strong_depth + self.html_block_strong_depth > 0;
        self.active_style.strikethrough =
            self.strikethrough_depth + self.html_block_strikethrough_depth > 0;
        self.active_style.code = self.code_depth + self.html_block_code_depth > 0;
    }

    fn reset_inline_style_state(&mut self) {
        self.emphasis_depth = 0;
        self.strong_depth = 0;
        self.strikethrough_depth = 0;
        self.code_depth = 0;
        self.html_link_stack.clear();
        self.refresh_inline_style_flags();
        self.refresh_link_style();
    }

    fn active_link_context(&self) -> Option<&LinkContext> {
        self.html_link_stack
            .iter()
            .rev()
            .find_map(Option::as_ref)
            .or_else(|| {
                self.html_block_link_stack
                    .iter()
                    .rev()
                    .find_map(Option::as_ref)
            })
            .or_else(|| self.link_stack.last())
    }

    fn refresh_link_style(&mut self) {
        let link = self.active_link_context();
        let link_url = link
            .map(|link| link.url.clone())
            .or_else(|| self.image_stack.last().map(|image| image.dest_url.clone()));
        let link_title = link.and_then(|link| link.title.clone());

        self.active_style.link = link_url.is_some();
        self.active_style.link_url = link_url;
        self.active_style.link_title = link_title;
    }

    fn update_html_block_context(&mut self, html: &str) {
        if let Some(tag) = inline_html_link_tag(html) {
            match tag {
                InlineHtmlLinkTag::Open(link) => self.html_block_link_stack.push(link),
                InlineHtmlLinkTag::Close => {
                    self.html_block_link_stack.pop();
                }
            }
        }

        if let Some((tag, closing)) = inline_html_style_tag(html) {
            if closing {
                self.decrement_html_block_style(tag);
            } else {
                self.increment_html_block_style(tag);
            }
            self.refresh_inline_style_flags();
        }
        self.refresh_link_style();
    }

    fn increment_html_block_style(&mut self, tag: InlineHtmlStyleTag) {
        match tag {
            InlineHtmlStyleTag::Emphasis => self.html_block_emphasis_depth += 1,
            InlineHtmlStyleTag::Strong => self.html_block_strong_depth += 1,
            InlineHtmlStyleTag::Strikethrough => self.html_block_strikethrough_depth += 1,
            InlineHtmlStyleTag::Code => self.html_block_code_depth += 1,
        }
    }

    fn decrement_html_block_style(&mut self, tag: InlineHtmlStyleTag) {
        match tag {
            InlineHtmlStyleTag::Emphasis => {
                self.html_block_emphasis_depth = self.html_block_emphasis_depth.saturating_sub(1);
            }
            InlineHtmlStyleTag::Strong => {
                self.html_block_strong_depth = self.html_block_strong_depth.saturating_sub(1);
            }
            InlineHtmlStyleTag::Strikethrough => {
                self.html_block_strikethrough_depth =
                    self.html_block_strikethrough_depth.saturating_sub(1);
            }
            InlineHtmlStyleTag::Code => {
                self.html_block_code_depth = self.html_block_code_depth.saturating_sub(1);
            }
        }
    }
}

fn normalize_commonmark_source(source: &str) -> Cow<'_, str> {
    if source.contains('\0') {
        Cow::Owned(source.replace('\0', "\u{fffd}"))
    } else {
        Cow::Borrowed(source)
    }
}

fn nonempty_shared_string(text: &CowStr<'_>) -> Option<SharedString> {
    (!text.is_empty()).then(|| SharedString::from(text.to_string()))
}

fn link_url(link_type: LinkType, dest_url: &CowStr<'_>) -> SharedString {
    let url = commonmark_url(dest_url);
    match link_type {
        LinkType::Email => SharedString::from(format!("mailto:{url}")),
        _ => url,
    }
}

fn commonmark_url(url: &CowStr<'_>) -> SharedString {
    SharedString::from(escape_commonmark_url(url.as_ref()))
}

fn escape_commonmark_url(url: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut escaped = String::new();
    for byte in url.bytes() {
        if commonmark_url_byte_is_safe(byte) {
            escaped.push(byte as char);
        } else {
            escaped.push('%');
            escaped.push(HEX[(byte >> 4) as usize] as char);
            escaped.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    escaped
}

fn commonmark_url_byte_is_safe(byte: u8) -> bool {
    // Store navigable URLs, not quoted HTML attributes, so keep '&' intact.
    matches!(
        byte,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b'-'
            | b'.'
            | b'/'
            | b'0'..=b'9'
            | b':'
            | b';'
            | b'='
            | b'?'
            | b'@'
            | b'A'..=b'Z'
            | b'^'
            | b'_'
            | b'a'..=b'z'
            | b'~'
    )
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

fn render_document(
    document: MarkdownDocument,
    style: MarkdownStyle,
    helix_theme: Option<&helix_view::Theme>,
    syntax_loader: Option<&syntax::Loader>,
) -> gpui::Div {
    let gap = block_gap(&style);
    let elements = render_blocks(
        document.blocks,
        &style,
        helix_theme,
        syntax_loader,
        "markdown",
    );

    div()
        .flex()
        .flex_col()
        .flex_none()
        .w_full()
        .gap(gap)
        .children(elements)
}

fn block_gap(style: &MarkdownStyle) -> Pixels {
    if style.preview {
        px(0.0)
    } else if style.compact {
        px(6.0)
    } else {
        px(10.0)
    }
}

fn render_blocks(
    blocks: Vec<MarkdownBlock>,
    style: &MarkdownStyle,
    helix_theme: Option<&helix_view::Theme>,
    syntax_loader: Option<&syntax::Loader>,
    id_prefix: &str,
) -> Vec<gpui::AnyElement> {
    blocks
        .into_iter()
        .enumerate()
        .map(|(block_index, block)| match block {
            MarkdownBlock::Paragraph(text) => render_rich_text(
                text,
                style,
                style.body_color,
                format!("{id_prefix}-paragraph-{block_index}"),
            )
            .line_height(relative(if style.preview { 1.55 } else { 1.45 }))
            .when(style.preview, |this| this.mb(px(10.0)))
            .into_any_element(),
            MarkdownBlock::Heading { level, text } => {
                let heading = render_rich_text(
                    text,
                    style,
                    style.heading_color,
                    format!("{id_prefix}-heading-{block_index}"),
                )
                .font_weight(FontWeight::BOLD)
                .line_height(relative(1.2));

                if style.preview {
                    heading
                        .text_size(preview_heading_size(style, level))
                        .when(block_index > 0, |this| this.mt(px(22.0)))
                        .mb(px(10.0))
                        .when(level <= 3, |this| {
                            this.pb(px(5.0)).border_b_1().border_color(
                                style.heading_border_color.unwrap_or(style.rule_color),
                            )
                        })
                        .into_any_element()
                } else {
                    let size = match level {
                        1 => 1.16,
                        2 => 1.08,
                        _ => 1.0,
                    };
                    heading.text_size(rems(size)).into_any_element()
                }
            }
            MarkdownBlock::CodeBlock { language, text } => render_code_block(
                text,
                language,
                style,
                helix_theme,
                syntax_loader,
                &format!("{id_prefix}-code-block-{block_index}"),
            )
            .into_any_element(),
            MarkdownBlock::HtmlBlock { text } => render_html_block(
                text,
                style,
                &format!("{id_prefix}-html-block-{block_index}"),
            )
            .into_any_element(),
            MarkdownBlock::Image {
                url,
                alt,
                title,
                link_url,
                link_title,
            } => render_image_block(
                url,
                alt,
                title,
                link_url,
                link_title,
                style,
                &format!("{id_prefix}-image-{block_index}"),
            )
            .into_any_element(),
            MarkdownBlock::ListItem { .. } => render_list_item(
                block,
                style,
                helix_theme,
                syntax_loader,
                &format!("{id_prefix}-list-item-{block_index}"),
            )
            .into_any_element(),
            MarkdownBlock::BlockQuote { kind, blocks } => render_block_quote(
                kind,
                blocks,
                style,
                helix_theme,
                syntax_loader,
                &format!("{id_prefix}-block-quote-{block_index}"),
            )
            .into_any_element(),
            MarkdownBlock::Rule => div()
                .h(px(1.0))
                .w_full()
                .bg(style.rule_color)
                .when(style.preview, |this| this.my(px(14.0)))
                .into_any_element(),
            MarkdownBlock::Table { alignments, rows } => render_table(
                alignments,
                rows,
                style,
                &format!("{id_prefix}-table-{block_index}"),
            )
            .into_any_element(),
        })
        .collect()
}

fn preview_heading_size(style: &MarkdownStyle, level: u8) -> Pixels {
    let index = usize::from(level.saturating_sub(1)).min(style.heading_font_sizes.len() - 1);
    style.heading_font_sizes[index]
}

fn render_rich_text(
    text: RichText,
    style: &MarkdownStyle,
    color: Hsla,
    element_id: impl Into<gpui::ElementId>,
) -> gpui::Div {
    let element_id = element_id.into();
    if !text.inline_images().is_empty() {
        return render_rich_text_with_inline_images(text, style, color, element_id);
    }

    render_rich_text_fragment(text, style, element_id)
        .w_full()
        .text_size(style.body_font_size)
        .text_color(color)
}

fn render_rich_text_fragment(
    text: RichText,
    style: &MarkdownStyle,
    element_id: impl Into<gpui::ElementId>,
) -> gpui::Div {
    let parts = text.into_render_parts(style);
    let text = StyledText::new(visible_rich_text(&parts.text)).with_highlights(parts.highlights);
    let text = if parts.links.is_empty() {
        text.into_any_element()
    } else {
        let click_ranges = parts
            .links
            .iter()
            .map(|link| link.range.clone())
            .collect::<Vec<_>>();
        let click_links = parts.links.clone();
        let tooltip_links = parts.links;
        let tooltip_style = style.clone();

        InteractiveText::new(element_id, text)
            .on_click(click_ranges, move |range_ix, _window, cx| {
                let Some(link) = click_links.get(range_ix) else {
                    return;
                };

                let url = link.url.to_string();
                nucleotide_logging::info!(
                    url = %url,
                    "Markdown documentation link click received"
                );
                cx.open_url(&url);
            })
            .tooltip(move |index, _window, cx| {
                tooltip_links
                    .iter()
                    .find(|link| link.range.contains(&index))
                    .and_then(|link| link.title.clone())
                    .map(|title| {
                        cx.new(|_| MarkdownTooltip {
                            text: title,
                            style: tooltip_style.clone(),
                        })
                        .into()
                    })
            })
            .into_any_element()
    };

    div().child(text)
}

fn render_rich_text_with_inline_images(
    text: RichText,
    style: &MarkdownStyle,
    color: Hsla,
    element_id: ElementId,
) -> gpui::Div {
    let id_base = element_id.to_string();
    let mut children = Vec::new();
    let mut cursor = 0;

    for (image_index, image) in text.inline_images().iter().cloned().enumerate() {
        if cursor < image.range.start {
            let segment = text.slice(cursor..image.range.start);
            if !segment.is_empty() {
                children.push(
                    render_rich_text_fragment(
                        segment,
                        style,
                        format!("{id_base}-text-{image_index}"),
                    )
                    .into_any_element(),
                );
            }
        }

        cursor = image.range.end;
        children.push(
            render_inline_image(
                image,
                style,
                &format!("{id_base}-inline-image-{image_index}"),
            )
            .into_any_element(),
        );
    }

    if cursor < text.text_len() {
        let segment = text.slice(cursor..text.text_len());
        if !segment.is_empty() {
            children.push(
                render_rich_text_fragment(segment, style, format!("{id_base}-text-tail"))
                    .into_any_element(),
            );
        }
    }

    div()
        .w_full()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(4.0))
        .text_size(style.body_font_size)
        .text_color(color)
        .children(children)
}

fn visible_rich_text(content: &SharedString) -> SharedString {
    if content.is_empty() {
        SharedString::from(" ")
    } else {
        content.clone()
    }
}

fn code_syntax_highlights(
    content: &str,
    language: Option<&str>,
    helix_theme: Option<&helix_view::Theme>,
    syntax_loader: Option<&syntax::Loader>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let Some(language) = normalized_code_language(language) else {
        return Vec::new();
    };
    let (Some(theme), Some(loader)) = (helix_theme, syntax_loader) else {
        return Vec::new();
    };

    tree_sitter_code_highlights(content, &language, theme, loader).unwrap_or_default()
}

fn normalized_code_language(language: Option<&str>) -> Option<String> {
    language
        .map(str::trim)
        .map(|language| language.trim_start_matches('{').trim_end_matches('}'))
        .and_then(|language| {
            language
                .split(|ch: char| ch == ',' || ch.is_whitespace())
                .next()
        })
        .filter(|language| !language.is_empty())
        .map(ToOwned::to_owned)
}

fn tree_sitter_code_highlights(
    content: &str,
    language: &str,
    theme: &helix_view::Theme,
    loader: &syntax::Loader,
) -> Option<Vec<(Range<usize>, HighlightStyle)>> {
    let ropeslice = RopeSlice::from(content);
    let language = loader.language_for_match(RopeSlice::from(language))?;
    let syntax = Syntax::new(ropeslice, language, loader).ok()?;
    let mut highlighter = syntax.highlighter(ropeslice, loader, ..);
    let mut highlight_stack = Vec::new();
    let mut highlights = Vec::new();
    let mut position = 0;
    let end = ropeslice.len_bytes() as u32;

    while position < end {
        if position == highlighter.next_event_offset() {
            let (event, new_highlights) = highlighter.advance();
            if event == HighlightEvent::Refresh {
                highlight_stack.clear();
            }
            highlight_stack.extend(new_highlights);
        }

        let start = position;
        position = highlighter.next_event_offset();
        if position == u32::MAX {
            position = end;
        }
        if position == start {
            continue;
        }
        if position < start {
            return None;
        }

        let style = highlight_stack
            .iter()
            .fold(HelixStyle::default(), |acc, highlight| {
                acc.patch(safe_highlight(theme, *highlight))
            });

        if let Some(style) = helix_style_to_highlight_style(style) {
            let start = next_char_boundary(content, start as usize);
            let end = next_char_boundary(content, position as usize);
            if start < end {
                highlights.push((start..end, style));
            }
        }
    }

    Some(highlights)
}

fn safe_highlight(theme: &helix_view::Theme, highlight: syntax::Highlight) -> HelixStyle {
    catch_unwind(AssertUnwindSafe(|| theme.highlight(highlight))).unwrap_or_default()
}

fn helix_style_to_highlight_style(style: HelixStyle) -> Option<HighlightStyle> {
    let color = style.fg.and_then(color_to_hsla);
    let background_color = style.bg.and_then(color_to_hsla);
    let underline_color = style.underline_color.and_then(color_to_hsla).or(color);
    let underline = match style.underline_style {
        Some(HelixUnderlineStyle::Line)
        | Some(HelixUnderlineStyle::Dotted)
        | Some(HelixUnderlineStyle::Dashed)
        | Some(HelixUnderlineStyle::DoubleLine) => Some(UnderlineStyle {
            thickness: px(1.0),
            color: underline_color,
            wavy: false,
        }),
        Some(HelixUnderlineStyle::Curl) => Some(UnderlineStyle {
            thickness: px(1.0),
            color: underline_color,
            wavy: true,
        }),
        Some(HelixUnderlineStyle::Reset) | None => None,
    };
    let strikethrough = style
        .add_modifier
        .contains(HelixModifier::CROSSED_OUT)
        .then_some(StrikethroughStyle {
            thickness: px(1.0),
            color,
        });
    let font_weight = style
        .add_modifier
        .contains(HelixModifier::BOLD)
        .then_some(FontWeight::BOLD);
    let font_style = style
        .add_modifier
        .contains(HelixModifier::ITALIC)
        .then_some(FontStyle::Italic);
    let fade_out = style
        .add_modifier
        .contains(HelixModifier::DIM)
        .then_some(0.6);

    if color.is_none()
        && background_color.is_none()
        && underline.is_none()
        && strikethrough.is_none()
        && font_weight.is_none()
        && font_style.is_none()
        && fade_out.is_none()
    {
        return None;
    }

    Some(HighlightStyle {
        color,
        background_color,
        underline,
        strikethrough,
        font_weight,
        font_style,
        fade_out,
    })
}

fn next_char_boundary(content: &str, mut index: usize) -> usize {
    index = index.min(content.len());
    while index < content.len() && !content.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn render_code_block(
    text: String,
    language: Option<String>,
    style: &MarkdownStyle,
    helix_theme: Option<&helix_view::Theme>,
    syntax_loader: Option<&syntax::Loader>,
    block_id: &str,
) -> gpui::Div {
    let content = code_block_display_text(&text);
    let highlights =
        code_syntax_highlights(&content, language.as_deref(), helix_theme, syntax_loader);
    let code = div()
        .id(block_id.to_string())
        .w_full()
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
        .when_else(
            style.code_overflow_x_scroll,
            |this| this.overflow_x_scroll(),
            |this| this.overflow_hidden(),
        )
        .child(StyledText::new(visible_code_text(&content)).with_highlights(highlights));

    let block = if let Some(language) = language {
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
    };

    block.when(style.preview, |this| this.mt(px(8.0)).mb(px(14.0)))
}

fn code_block_display_text(text: &str) -> String {
    text.strip_suffix('\n').unwrap_or(text).to_string()
}

fn visible_code_text(content: &str) -> SharedString {
    if content.is_empty() {
        SharedString::from(" ")
    } else {
        SharedString::from(content.to_string())
    }
}

fn render_html_block(text: String, style: &MarkdownStyle, block_id: &str) -> impl IntoElement {
    let text = visible_html_text(&text);

    div()
        .id(block_id.to_string())
        .w_full()
        .when(text.is_empty(), |this| this.hidden())
        .when(!text.is_empty(), |this| {
            this.text_size(style.body_font_size)
                .text_color(style.body_color)
                .line_height(relative(if style.preview { 1.55 } else { 1.45 }))
                .when(style.preview, |this| this.mb(px(10.0)))
                .child(StyledText::new(text))
        })
}

fn visible_html_text(content: &str) -> SharedString {
    let text = html_display_text(content);
    SharedString::from(text)
}

fn html_display_text(content: &str) -> String {
    let mut text = strip_html_markup(content);
    let trimmed_len = text.trim_matches(['\n', '\r']).len();
    if trimmed_len != text.len() {
        text = text.trim_matches(['\n', '\r']).to_string();
    }
    text
}

fn strip_html_markup(content: &str) -> String {
    let mut text = String::new();
    let mut cursor = 0;

    while cursor < content.len() {
        let rest = &content[cursor..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            if let Some(end) = comment.find("-->") {
                cursor += "<!--".len() + end + "-->".len();
            } else {
                break;
            }
        } else if let Some(rest) = rest.strip_prefix("<![CDATA[") {
            if let Some(end) = rest.find("]]>") {
                text.push_str(&rest[..end]);
                cursor += "<![CDATA[".len() + end + "]]>".len();
            } else {
                text.push_str(rest);
                break;
            }
        } else if let Some(instruction) = rest.strip_prefix("<?") {
            if let Some(end) = instruction.find("?>") {
                cursor += "<?".len() + end + "?>".len();
            } else {
                break;
            }
        } else if let Some(declaration) = html_declaration(rest) {
            if let Some(end) = declaration.find('>') {
                cursor += "<!".len() + end + ">".len();
            } else {
                break;
            }
        } else if let Some(tag_name) = hidden_html_raw_text_element_name(rest) {
            if let Some(skip_len) = html_raw_text_element_len(rest, tag_name) {
                cursor += skip_len;
            } else {
                text.push('<');
                cursor += '<'.len_utf8();
            }
        } else if rest.starts_with('<') {
            if let Some(tag_len) = html_normal_tag_len(rest) {
                cursor += tag_len;
            } else {
                text.push('<');
                cursor += '<'.len_utf8();
            }
        } else if let Some(next_tag_start) = rest.find('<') {
            push_visible_html_text(&mut text, &rest[..next_tag_start]);
            cursor += next_tag_start;
        } else {
            push_visible_html_text(&mut text, rest);
            break;
        }
    }

    text
}

fn push_visible_html_text(text: &mut String, content: &str) {
    let mut cursor = 0;

    while cursor < content.len() {
        let rest = &content[cursor..];
        let Some(entity_start) = rest.find('&') else {
            text.push_str(rest);
            break;
        };

        text.push_str(&rest[..entity_start]);
        let candidate = &rest[entity_start..];
        let Some(entity_len) = html_entity_candidate_len(candidate) else {
            text.push('&');
            cursor += entity_start + '&'.len_utf8();
            continue;
        };

        let entity = &candidate[..entity_len];
        if let Some(decoded) = decode_commonmark_entity(entity) {
            text.push_str(&decoded);
        } else {
            text.push_str(entity);
        }
        cursor += entity_start + entity_len;
    }
}

fn decode_visible_html_entities(content: &str) -> String {
    let mut text = String::new();
    push_visible_html_text(&mut text, content);
    text
}

fn html_entity_candidate_len(content: &str) -> Option<usize> {
    let content = content.strip_prefix('&')?;
    let mut chars = content.char_indices();
    let (_, first) = chars.next()?;
    if first == '#' {
        return numeric_html_entity_len(content).map(|len| '&'.len_utf8() + len);
    }
    if !first.is_ascii_alphanumeric() {
        return None;
    }

    for (index, ch) in chars {
        if ch == ';' {
            return Some('&'.len_utf8() + index + ch.len_utf8());
        }
        if !ch.is_ascii_alphanumeric() {
            return None;
        }
    }

    None
}

fn numeric_html_entity_len(content: &str) -> Option<usize> {
    let digits = content
        .strip_prefix("#x")
        .or_else(|| content.strip_prefix("#X"));
    if let Some(digits) = digits {
        let len = digits.find(';').filter(|&index| {
            index > 0 && digits[..index].chars().all(|ch| ch.is_ascii_hexdigit())
        })?;
        return Some("#x".len() + len + ';'.len_utf8());
    }

    let digits = content.strip_prefix('#')?;
    let len = digits
        .find(';')
        .filter(|&index| index > 0 && digits[..index].chars().all(|ch| ch.is_ascii_digit()))?;
    Some('#'.len_utf8() + len + ';'.len_utf8())
}

fn decode_commonmark_entity(entity: &str) -> Option<String> {
    let mut decoded = String::new();
    for event in Parser::new(entity) {
        match event {
            Event::Text(text) => decoded.push_str(text.as_ref()),
            Event::Start(Tag::Paragraph) | Event::End(TagEnd::Paragraph) => {}
            _ => return None,
        }
    }

    (decoded != entity).then_some(decoded)
}

fn html_declaration(markup: &str) -> Option<&str> {
    let declaration = markup.strip_prefix("<!")?;
    declaration
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic())
        .then_some(declaration)
}

fn hidden_html_raw_text_element_name(markup: &str) -> Option<&'static str> {
    let tag_name = html_open_tag_name(markup)?;
    if tag_name.eq_ignore_ascii_case("script") {
        Some("script")
    } else if tag_name.eq_ignore_ascii_case("style") {
        Some("style")
    } else {
        None
    }
}

fn html_open_tag_name(markup: &str) -> Option<&str> {
    let body = markup.strip_prefix('<')?;
    let mut chars = body.char_indices();
    let (_, first) = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }

    for (index, ch) in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '-') {
            return Some(&body[..index]);
        }
    }

    Some(body)
}

fn html_raw_text_element_len(markup: &str, tag_name: &str) -> Option<usize> {
    let after_opening_start = html_normal_tag_len(markup)?;
    let after_opening = &markup[after_opening_start..];
    let Some(closing_start) = find_closing_html_tag(after_opening, tag_name) else {
        return Some(markup.len());
    };
    let closing = &after_opening[closing_start..];
    let closing_len = html_normal_tag_len(closing)?;
    Some(after_opening_start + closing_start + closing_len)
}

fn html_normal_tag_len(markup: &str) -> Option<usize> {
    let tag_len = html_tag_len(markup)?;
    let tag = &markup[..tag_len];
    if html_open_tag_is_valid(tag) || html_closing_tag_is_valid(tag) {
        Some(tag_len)
    } else {
        None
    }
}

fn html_tag_len(markup: &str) -> Option<usize> {
    if !markup.starts_with('<') {
        return None;
    }

    let mut quote = None;
    for (index, ch) in markup.char_indices().skip(1) {
        match quote {
            Some(active_quote) if ch == active_quote => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch == '>' => return Some(index + ch.len_utf8()),
            None => {}
        }
    }

    None
}

fn html_open_tag_is_valid(tag: &str) -> bool {
    let Some(mut rest) = tag.strip_prefix('<').and_then(|tag| tag.strip_suffix('>')) else {
        return false;
    };
    if rest.starts_with(['/', '!', '?']) {
        return false;
    }

    let Some(tag_name_len) = html_tag_name_len(rest) else {
        return false;
    };
    rest = &rest[tag_name_len..];

    loop {
        rest = trim_html_whitespace(rest);
        if rest.is_empty() {
            return true;
        }
        if rest == "/" {
            return true;
        }
        if rest.starts_with('/') {
            return false;
        }

        let Some(attribute_name_len) = html_attribute_name_len(rest) else {
            return false;
        };
        rest = &rest[attribute_name_len..];
        let after_name = trim_html_whitespace(rest);
        if let Some(after_equals) = after_name.strip_prefix('=') {
            let after_equals = trim_html_whitespace(after_equals);
            let Some(value_len) = html_attribute_value_len(after_equals) else {
                return false;
            };
            rest = &after_equals[value_len..];
        }

        if !html_tag_attribute_separator_is_next(rest) {
            return false;
        }
    }
}

fn html_closing_tag_is_valid(tag: &str) -> bool {
    let Some(body) = tag.strip_prefix("</").and_then(|tag| tag.strip_suffix('>')) else {
        return false;
    };
    let body = trim_html_whitespace(body);
    let Some(tag_name_len) = html_tag_name_len(body) else {
        return false;
    };

    trim_html_whitespace(&body[tag_name_len..]).is_empty()
}

fn html_tag_name_len(text: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }

    for (index, ch) in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '-') {
            return Some(index);
        }
    }

    Some(text.len())
}

fn html_attribute_name_len(text: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_' || first == ':') {
        return None;
    }

    for (index, ch) in chars {
        if !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '-')) {
            return Some(index);
        }
    }

    Some(text.len())
}

fn html_attribute_value_len(text: &str) -> Option<usize> {
    if let Some(value) = text.strip_prefix('"') {
        return value
            .find('"')
            .map(|index| '"'.len_utf8() + index + '"'.len_utf8());
    }
    if let Some(value) = text.strip_prefix('\'') {
        return value
            .find('\'')
            .map(|index| '\''.len_utf8() + index + '\''.len_utf8());
    }

    let mut len = 0;
    for (index, ch) in text.char_indices() {
        if ch.is_ascii_whitespace() || matches!(ch, '"' | '\'' | '=' | '<' | '>' | '`') {
            break;
        }
        len = index + ch.len_utf8();
    }

    (len > 0).then_some(len)
}

fn html_tag_attribute_separator_is_next(text: &str) -> bool {
    text.is_empty()
        || text.starts_with('/')
        || text
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_whitespace())
}

fn trim_html_whitespace(text: &str) -> &str {
    text.trim_matches(|ch: char| ch.is_ascii_whitespace())
}

fn find_closing_html_tag(content: &str, tag_name: &str) -> Option<usize> {
    let mut cursor = 0;

    while let Some(relative_start) = content[cursor..].find("</") {
        let start = cursor + relative_start;
        let after_slash = &content[start + "</".len()..];
        let Some(candidate) = after_slash.get(..tag_name.len()) else {
            cursor = start + "</".len();
            continue;
        };
        if candidate.eq_ignore_ascii_case(tag_name)
            && after_slash[tag_name.len()..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_whitespace() || ch == '>')
        {
            return Some(start);
        }
        cursor = start + "</".len();
    }

    None
}

fn inline_html_is_line_break(html: &str) -> bool {
    let html = html.trim();
    let Some(body) = html
        .strip_prefix('<')
        .and_then(|html| html.strip_suffix('>'))
    else {
        return false;
    };

    if body.starts_with(['/', '!', '?']) {
        return false;
    }

    let tag_name = body
        .split(|ch: char| ch.is_ascii_whitespace() || ch == '/')
        .next()
        .unwrap_or_default();
    tag_name.eq_ignore_ascii_case("br")
}

#[derive(Clone)]
enum InlineHtmlLinkTag {
    Open(Option<LinkContext>),
    Close,
}

fn inline_html_link_tag(html: &str) -> Option<InlineHtmlLinkTag> {
    let body = html.trim().strip_prefix('<')?.strip_suffix('>')?.trim();
    if body.trim_end().ends_with('/') {
        return None;
    }

    let (closing, body) = if let Some(body) = body.strip_prefix('/') {
        (true, body.trim_start())
    } else {
        (false, body)
    };
    let tag_name_len = body
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .unwrap_or(body.len());
    if tag_name_len == 0 || !body[..tag_name_len].eq_ignore_ascii_case("a") {
        return None;
    }

    let rest = trim_html_whitespace(&body[tag_name_len..]);
    if closing {
        return rest.is_empty().then_some(InlineHtmlLinkTag::Close);
    }

    let (href, title) = html_link_attributes(rest)?;
    Some(InlineHtmlLinkTag::Open(href.map(|url| LinkContext {
        url: SharedString::from(url),
        title: title.map(SharedString::from),
    })))
}

fn html_link_attributes(mut rest: &str) -> Option<(Option<String>, Option<String>)> {
    let mut href = None;
    let mut title = None;

    loop {
        rest = trim_html_whitespace(rest);
        if rest.is_empty() {
            return Some((href, title));
        }
        if rest == "/" {
            return None;
        }
        if rest.starts_with('/') {
            return None;
        }

        let name_len = html_attribute_name_len(rest)?;
        let name = &rest[..name_len];
        rest = &rest[name_len..];
        let after_name = trim_html_whitespace(rest);
        if let Some(after_equals) = after_name.strip_prefix('=') {
            let after_equals = trim_html_whitespace(after_equals);
            let value_len = html_attribute_value_len(after_equals)?;
            let value = html_attribute_decoded_value(&after_equals[..value_len]);
            if name.eq_ignore_ascii_case("href") {
                href = Some(value);
            } else if name.eq_ignore_ascii_case("title") {
                title = Some(value);
            }
            rest = &after_equals[value_len..];
        }

        if !html_tag_attribute_separator_is_next(rest) {
            return None;
        }
    }
}

fn html_attribute_decoded_value(value: &str) -> String {
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value);
    decode_visible_html_entities(value)
}

#[derive(Clone, Copy)]
enum InlineHtmlStyleTag {
    Emphasis,
    Strong,
    Strikethrough,
    Code,
}

fn inline_html_style_tag(html: &str) -> Option<(InlineHtmlStyleTag, bool)> {
    let body = html.trim().strip_prefix('<')?.strip_suffix('>')?.trim();
    if body.trim_end().ends_with('/') {
        return None;
    }

    let (closing, body) = if let Some(body) = body.strip_prefix('/') {
        (true, body.trim_start())
    } else {
        (false, body)
    };
    let tag_name_len = body
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .unwrap_or(body.len());
    if tag_name_len == 0 {
        return None;
    }

    let tag_name = &body[..tag_name_len];
    let tag = if tag_name.eq_ignore_ascii_case("em") || tag_name.eq_ignore_ascii_case("i") {
        InlineHtmlStyleTag::Emphasis
    } else if tag_name.eq_ignore_ascii_case("strong") || tag_name.eq_ignore_ascii_case("b") {
        InlineHtmlStyleTag::Strong
    } else if tag_name.eq_ignore_ascii_case("del")
        || tag_name.eq_ignore_ascii_case("s")
        || tag_name.eq_ignore_ascii_case("strike")
    {
        InlineHtmlStyleTag::Strikethrough
    } else if tag_name.eq_ignore_ascii_case("code") || tag_name.eq_ignore_ascii_case("kbd") {
        InlineHtmlStyleTag::Code
    } else {
        return None;
    };

    Some((tag, closing))
}

fn render_image_block(
    url: SharedString,
    alt: RichText,
    title: Option<SharedString>,
    link_url: Option<SharedString>,
    link_title: Option<SharedString>,
    style: &MarkdownStyle,
    block_id: &str,
) -> impl IntoElement {
    let fallback_style = style.clone();
    let fallback_id = format!("{block_id}-fallback");
    let fallback_link_url = link_url.clone();
    let fallback_link_title = link_title.clone();
    let tooltip_title = title.or_else(|| link_title.clone());
    let fallback_text = image_fallback_text(&url, alt, fallback_link_url, fallback_link_title);

    div()
        .id(block_id.to_string())
        .w_full()
        .when(style.preview, |this| this.my(px(10.0)))
        .when_some(tooltip_title, |this, title| {
            let tooltip_style = style.clone();
            this.tooltip(move |_window, cx| {
                cx.new(|_| MarkdownTooltip {
                    text: title.clone(),
                    style: tooltip_style.clone(),
                })
                .into()
            })
        })
        .when_some(link_url, |this, link_url| {
            let link_url = link_url.to_string();
            this.cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                    cx.stop_propagation();
                    cx.open_url(&link_url);
                })
        })
        .child(
            img(url)
                .max_w_full()
                .rounded(px(4.0))
                .with_fallback(move || {
                    render_rich_text(
                        fallback_text.clone(),
                        &fallback_style,
                        fallback_style.body_color,
                        fallback_id.clone(),
                    )
                    .into_any_element()
                }),
        )
}

fn render_inline_image(
    image: InlineImage,
    style: &MarkdownStyle,
    image_id: &str,
) -> impl IntoElement {
    let fallback_style = style.clone();
    let fallback_id = format!("{image_id}-fallback");
    let link_url = image.link_url.clone();
    let link_title = image.link_title.clone();
    let title = image.title.clone().or_else(|| image.link_title.clone());
    let url = image.url.clone();
    let fallback_text = image_fallback_text(&url, image.alt, link_url.clone(), link_title);

    div()
        .id(image_id.to_string())
        .flex()
        .items_center()
        .when_some(title, |this, title| {
            let tooltip_style = style.clone();
            this.tooltip(move |_window, cx| {
                cx.new(|_| MarkdownTooltip {
                    text: title.clone(),
                    style: tooltip_style.clone(),
                })
                .into()
            })
        })
        .when_some(link_url, |this, link_url| {
            let link_url = link_url.to_string();
            this.cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                    cx.stop_propagation();
                    cx.open_url(&link_url);
                })
        })
        .child(
            img(url)
                .max_w_full()
                .rounded(px(4.0))
                .with_fallback(move || {
                    render_rich_text_fragment(
                        fallback_text.clone(),
                        &fallback_style,
                        fallback_id.clone(),
                    )
                    .text_size(fallback_style.body_font_size)
                    .text_color(fallback_style.body_color)
                    .into_any_element()
                }),
        )
}

fn image_fallback_text(
    url: &SharedString,
    alt: RichText,
    link_url: Option<SharedString>,
    link_title: Option<SharedString>,
) -> RichText {
    if !alt.is_empty() {
        if let Some(link_url) = link_url {
            return linked_rich_text(alt, link_url, link_title);
        }
        return alt;
    }

    let mut text = RichText::default();
    text.push(
        url.as_ref(),
        InlineStyle {
            link: true,
            link_url: Some(link_url.unwrap_or_else(|| url.clone())),
            link_title,
            ..InlineStyle::default()
        },
    );
    text
}

fn linked_rich_text(
    mut text: RichText,
    link_url: SharedString,
    link_title: Option<SharedString>,
) -> RichText {
    for span in &mut text.spans {
        span.style.link = true;
        span.style.link_url = Some(link_url.clone());
        span.style.link_title = link_title.clone();
    }
    text
}

fn rich_text_from_plain(text: impl AsRef<str>) -> RichText {
    let mut rich_text = RichText::default();
    rich_text.push(text.as_ref(), InlineStyle::default());
    rich_text
}

fn render_list_item(
    block: MarkdownBlock,
    style: &MarkdownStyle,
    helix_theme: Option<&helix_view::Theme>,
    syntax_loader: Option<&syntax::Loader>,
    block_id: &str,
) -> gpui::Div {
    let MarkdownBlock::ListItem {
        ordered,
        index,
        depth,
        checked,
        text,
        children,
    } = block
    else {
        unreachable!("render_list_item only accepts list item blocks");
    };

    let marker_width = if checked.is_some() || !ordered {
        px(24.0)
    } else {
        px(34.0)
    };
    let marker = if let Some(checked) = checked {
        render_task_checkbox(checked, style).into_any_element()
    } else if ordered {
        div()
            .w(marker_width)
            .text_sm()
            .text_right()
            .text_color(style.secondary_color)
            .child(format!("{index}."))
            .into_any_element()
    } else {
        div()
            .w(marker_width)
            .text_sm()
            .text_center()
            .text_color(style.secondary_color)
            .child(bullet_for_depth(depth))
            .into_any_element()
    };
    let child_blocks = render_blocks(
        children,
        style,
        helix_theme,
        syntax_loader,
        &format!("{block_id}-child"),
    );
    let needs_empty_placeholder =
        list_item_needs_empty_placeholder(&text, !child_blocks.is_empty());
    let content = if text.is_empty() {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .gap(block_gap(style))
            .when(needs_empty_placeholder, |this| {
                this.child(div().h(style.body_font_size))
            })
            .children(child_blocks)
    } else {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .gap(block_gap(style))
            .child(render_rich_text(
                text,
                style,
                style.body_color,
                block_id.to_string(),
            ))
            .children(child_blocks)
    };

    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .pl(list_item_padding(depth))
        .when(style.preview, |this| this.mb(px(6.0)))
        .child(div().flex_none().w(marker_width).child(marker))
        .child(content)
}

fn list_item_needs_empty_placeholder(text: &RichText, has_child_blocks: bool) -> bool {
    text.is_empty() && !has_child_blocks
}

fn list_item_padding(depth: usize) -> Pixels {
    if depth == 0 { px(0.0) } else { px(16.0) }
}

fn bullet_for_depth(depth: usize) -> &'static str {
    const BULLETS: [&str; 4] = ["•", "◦", "▪", "‣"];
    BULLETS[depth.min(BULLETS.len() - 1)]
}

fn render_task_checkbox(checked: bool, style: &MarkdownStyle) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(14.0))
        .h(px(14.0))
        .mt(px(2.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(if checked {
            style.link_color
        } else {
            style.code_border
        })
        .when(checked, |this| this.bg(with_alpha(style.link_color, 0.22)))
        .child(
            div()
                .text_size(px(10.0))
                .line_height(relative(1.0))
                .text_color(style.link_color)
                .child(if checked { "✓" } else { "" }),
        )
}

fn render_block_quote(
    kind: Option<MarkdownAlertKind>,
    blocks: Vec<MarkdownBlock>,
    style: &MarkdownStyle,
    helix_theme: Option<&helix_view::Theme>,
    syntax_loader: Option<&syntax::Loader>,
    block_id: &str,
) -> impl IntoElement {
    let border_color = kind
        .map(|kind| kind.color(style))
        .unwrap_or(style.quote_border);
    let content = render_blocks(
        blocks,
        style,
        helix_theme,
        syntax_loader,
        &format!("{block_id}-content"),
    );
    let is_empty = content.is_empty();

    div()
        .flex()
        .flex_col()
        .gap(block_gap(style))
        .pl(px(if kind.is_some() { 14.0 } else { 10.0 }))
        .py(px(if style.preview { 6.0 } else { 0.0 }))
        .when(style.preview, |this| this.mb(px(10.0)))
        .when(kind.is_some(), |this| {
            this.bg(with_alpha(border_color, 0.08))
                .rounded(px(4.0))
                .pr(px(10.0))
        })
        .border_l(px(if kind.is_some() { 4.0 } else { 2.0 }))
        .border_color(border_color)
        .when_some(kind, |this, kind| {
            this.child(
                div()
                    .mb(px(4.0))
                    .text_size(style.body_font_size)
                    .font_weight(FontWeight::BOLD)
                    .text_color(border_color)
                    .child(kind.label()),
            )
        })
        .when(is_empty, |this| this.child(div().h(style.body_font_size)))
        .children(content)
}

fn render_table(
    alignments: Vec<TableAlignment>,
    rows: Vec<Vec<RichText>>,
    style: &MarkdownStyle,
    block_id: &str,
) -> impl IntoElement {
    let column_count = rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(alignments.len());
    if column_count == 0 {
        return div();
    }

    let grid_cols = column_count.min(u16::MAX as usize) as u16;
    let mut cells = Vec::new();
    for (row_index, row) in rows.into_iter().enumerate() {
        let header = row_index == 0;
        for column_index in 0..column_count {
            let cell = row.get(column_index).cloned().unwrap_or_default();
            let alignment = alignments
                .get(column_index)
                .copied()
                .unwrap_or(TableAlignment::None);
            let text = render_rich_text(
                cell,
                style,
                style.body_color,
                format!("{block_id}-{row_index}-{column_index}"),
            )
            .px(px(8.0))
            .py(px(6.0))
            .when(header, |this| {
                this.font_weight(FontWeight::BOLD).text_center()
            })
            .when(!header, |this| match alignment {
                TableAlignment::None | TableAlignment::Left => this,
                TableAlignment::Center => this.text_center(),
                TableAlignment::Right => this.text_right(),
            });

            cells.push(
                div()
                    .flex()
                    .flex_col()
                    .border_color(style.code_border)
                    .when(column_index > 0, |this| this.border_l_1())
                    .when(row_index > 0, |this| this.border_t_1())
                    .when(header, |this| this.bg(style.table_header_background))
                    .when(!header && row_index % 2 == 1, |this| {
                        this.bg(style.table_alternate_background)
                    })
                    .child(text)
                    .into_any_element(),
            );
        }
    }

    div()
        .grid()
        .grid_cols(grid_cols)
        .w_full()
        .border_1()
        .border_color(style.code_border)
        .rounded(px(4.0))
        .overflow_hidden()
        .when(style.preview, |this| this.my(px(12.0)))
        .children(cells)
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
            MarkdownBlock::ListItem { ordered: false, index: 1, text, children, .. }
                if text.plain_text() == "fast"
                    && children.is_empty()
        ));
    }

    #[test]
    fn empty_atx_headings_are_preserved() {
        let document = MarkdownDocument::parse("##\n#\n### ###");

        assert_eq!(document.blocks.len(), 3);
        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::Heading { level: 2, text } if text.is_empty()
        ));
        assert!(matches!(
            &document.blocks[1],
            MarkdownBlock::Heading { level: 1, text } if text.is_empty()
        ));
        assert!(matches!(
            &document.blocks[2],
            MarkdownBlock::Heading { level: 3, text } if text.is_empty()
        ));
    }

    #[test]
    fn empty_rich_text_uses_visible_placeholder() {
        assert_eq!(
            visible_rich_text(&SharedString::from("")),
            SharedString::from(" ")
        );
        assert_eq!(
            visible_rich_text(&SharedString::from("heading")),
            SharedString::from("heading")
        );
    }

    #[test]
    fn rich_text_slice_clamps_to_utf8_boundaries() {
        let mut text = RichText::default();
        text.push("éx", InlineStyle::default());

        assert_eq!(text.slice(2..3).plain_text(), "x");
        assert_eq!(text.slice(1..3).plain_text(), "x");
        assert_eq!(text.slice(0..1).plain_text(), "");
    }

    #[test]
    fn empty_inline_link_text_preserves_paragraphs() {
        let document = MarkdownDocument::parse("[](./target.md)\n\n[]()");

        assert_eq!(document.blocks.len(), 2);
        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::Paragraph(text) if text.is_empty()
        ));
        assert!(matches!(
            &document.blocks[1],
            MarkdownBlock::Paragraph(text) if text.is_empty()
        ));
    }

    #[test]
    fn null_characters_are_replaced_before_parsing() {
        let document = MarkdownDocument::parse("a\0b\n\n```\n\0\n```");

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::Paragraph(text) if text.plain_text() == "a\u{fffd}b"
        ));
        assert!(matches!(
            &document.blocks[1],
            MarkdownBlock::CodeBlock { text, .. } if text == "\u{fffd}\n"
        ));
    }

    #[test]
    fn source_without_nulls_is_borrowed_unchanged() {
        assert!(matches!(
            normalize_commonmark_source("plain markdown"),
            Cow::Borrowed("plain markdown")
        ));
        assert_eq!(
            normalize_commonmark_source("has\0null").as_ref(),
            "has\u{fffd}null"
        );
    }

    #[test]
    fn parses_extended_links_task_lists_quotes_and_tables() {
        let document = MarkdownDocument::parse_extended(
            "> See [docs](https://example.com)\n\n- [x] done\n- [ ] next\n\n| A | B |\n| :- | -: |\n| left | right |",
        );

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::BlockQuote { kind: None, blocks }
                if matches!(
                    &blocks[0],
                    MarkdownBlock::Paragraph(text)
                        if text.spans().iter().any(|span| span.style.link
                            && span.style.link_url.as_ref().is_some_and(|url| url == "https://example.com"))
                )
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
                    && rows[0].iter().map(RichText::plain_text).collect::<Vec<_>>() == ["A", "B"]
                    && rows[1].iter().map(RichText::plain_text).collect::<Vec<_>>() == ["left", "right"]
        ));
    }

    #[test]
    fn parses_extended_gfm_alert_block_quotes() {
        let document = MarkdownDocument::parse_extended("> [!WARNING]\n> Check this carefully.");

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::BlockQuote {
                kind: Some(MarkdownAlertKind::Warning),
                blocks,
            } if matches!(
                &blocks[0],
                MarkdownBlock::Paragraph(text) if text.plain_text() == "Check this carefully."
            )
        ));
    }

    #[test]
    fn commonmark_parse_keeps_extension_syntax_literal() {
        let document = MarkdownDocument::parse(
            "> [!WARNING]\n> Check this carefully.\n\n- [x] done\n\n| A | B |\n| :- | -: |\n\n~~strike~~",
        );

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::BlockQuote { kind: None, blocks }
                if matches!(
                    &blocks[0],
                    MarkdownBlock::Paragraph(text)
                        if text.plain_text() == "[!WARNING] Check this carefully."
                )
        ));
        assert!(matches!(
            &document.blocks[1],
            MarkdownBlock::ListItem { checked: None, text, .. }
                if text.plain_text() == "[x] done"
        ));
        assert!(matches!(
            &document.blocks[2],
            MarkdownBlock::Paragraph(text)
                if text.plain_text() == "| A | B | | :- | -: |"
        ));
        assert!(matches!(
            &document.blocks[3],
            MarkdownBlock::Paragraph(text)
                if text.plain_text() == "~~strike~~"
                    && text.spans().iter().all(|span| !span.style.strikethrough)
        ));
    }

    #[test]
    fn nested_unordered_lists_keep_parent_and_child_items() {
        let document = MarkdownDocument::parse("- parent\n  - child\n    - grandchild");

        let MarkdownBlock::ListItem {
            text,
            children,
            depth,
            ..
        } = &document.blocks[0]
        else {
            panic!("expected parent list item");
        };

        assert_eq!(*depth, 0);
        assert_eq!(text.plain_text(), "parent");
        assert_eq!(children.len(), 1);

        let MarkdownBlock::ListItem {
            text,
            children,
            depth,
            ..
        } = &children[0]
        else {
            panic!("expected child list item");
        };

        assert_eq!(*depth, 1);
        assert_eq!(text.plain_text(), "child");
        assert_eq!(children.len(), 1);

        assert!(matches!(
            &children[0],
            MarkdownBlock::ListItem { text, depth: 2, children, .. }
                if text.plain_text() == "grandchild" && children.is_empty()
        ));
    }

    #[test]
    fn nested_unordered_list_render_indent_is_relative() {
        let document = MarkdownDocument::parse(" - foo\n   - bar\n\t - baz\n");

        assert!(matches!(
            document.blocks.as_slice(),
            [MarkdownBlock::ListItem { children, .. }]
                if matches!(
                    children.as_slice(),
                    [MarkdownBlock::ListItem { children, .. }]
                        if matches!(children.as_slice(), [MarkdownBlock::ListItem { .. }])
                )
        ));
        assert_eq!(list_item_padding(0), px(0.0));
        assert_eq!(list_item_padding(1), px(16.0));
        assert_eq!(list_item_padding(2), px(16.0));
        assert_eq!(list_item_padding(4), px(16.0));
    }

    #[test]
    fn loose_list_items_keep_continuation_blocks() {
        let document = MarkdownDocument::parse(
            "- first paragraph\n\n  second paragraph\n\n  ```\n  code\n  ```",
        );

        let MarkdownBlock::ListItem { text, children, .. } = &document.blocks[0] else {
            panic!("expected list item");
        };

        assert_eq!(text.plain_text(), "first paragraph");
        assert_eq!(children.len(), 2);
        assert!(matches!(
            &children[0],
            MarkdownBlock::Paragraph(text) if text.plain_text() == "second paragraph"
        ));
        assert!(matches!(
            &children[1],
            MarkdownBlock::CodeBlock { language: None, text } if text == "code\n"
        ));
    }

    #[test]
    fn commonmark_lazy_list_continuations_stay_in_item_text() {
        for source in ["- foo\nbar", "- foo\n  bar"] {
            let document = MarkdownDocument::parse(source);

            assert!(matches!(
                document.blocks.as_slice(),
                [MarkdownBlock::ListItem { text, children, .. }]
                    if text.plain_text() == "foo bar" && children.is_empty()
            ));
        }
    }

    #[test]
    fn commonmark_list_continuation_paragraph_indent_controls_nesting() {
        let document = MarkdownDocument::parse("- one\n\n two");

        assert!(matches!(
            document.blocks.as_slice(),
            [
                MarkdownBlock::ListItem { text, children, .. },
                MarkdownBlock::Paragraph(paragraph),
            ] if text.plain_text() == "one"
                && children.is_empty()
                && paragraph.plain_text() == "two"
        ));

        let document = MarkdownDocument::parse("- one\n\n  two");

        assert!(matches!(
            document.blocks.as_slice(),
            [MarkdownBlock::ListItem { text, children, .. }]
                if text.plain_text() == "one"
                    && matches!(
                        children.as_slice(),
                        [MarkdownBlock::Paragraph(paragraph)] if paragraph.plain_text() == "two"
                    )
        ));
    }

    #[test]
    fn commonmark_list_items_can_contain_only_child_blocks() {
        let document = MarkdownDocument::parse("-\n  foo\n-\n  ```\n  bar\n  ```\n-\n      baz");

        assert_eq!(document.blocks.len(), 3);
        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::ListItem { text, children, .. }
                if text.plain_text() == "foo" && children.is_empty()
        ));
        assert!(matches!(
            &document.blocks[1],
            MarkdownBlock::ListItem { text, children, .. }
                if text.is_empty()
                    && matches!(
                        children.as_slice(),
                        [MarkdownBlock::CodeBlock { language: None, text }] if text == "bar\n"
                    )
        ));
        assert!(matches!(
            &document.blocks[2],
            MarkdownBlock::ListItem { text, children, .. }
                if text.is_empty()
                    && matches!(
                        children.as_slice(),
                        [MarkdownBlock::CodeBlock { language: None, text }] if text == "baz"
                    )
        ));
    }

    #[test]
    fn block_quotes_keep_nested_blocks() {
        let document = MarkdownDocument::parse("> # Heading\n>\n> - quoted\n>   - nested");

        let MarkdownBlock::BlockQuote { blocks, .. } = &document.blocks[0] else {
            panic!("expected block quote");
        };

        assert_eq!(blocks.len(), 2);
        assert!(matches!(
            &blocks[0],
            MarkdownBlock::Heading { level: 1, text } if text.plain_text() == "Heading"
        ));
        assert!(matches!(
            &blocks[1],
            MarkdownBlock::ListItem { text, children, .. }
                if text.plain_text() == "quoted"
                    && matches!(
                        &children[0],
                        MarkdownBlock::ListItem { text, .. } if text.plain_text() == "nested"
                    )
        ));
    }

    #[test]
    fn list_items_can_start_with_block_children() {
        let document = MarkdownDocument::parse("- # Heading");

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::ListItem { text, children, .. }
                if text.is_empty()
                    && matches!(
                        children.as_slice(),
                        [MarkdownBlock::Heading { level: 1, text }] if text.plain_text() == "Heading"
                    )
        ));
    }

    #[test]
    fn list_item_paragraphs_after_child_blocks_keep_order() {
        let document = MarkdownDocument::parse("- # Heading\n\n  details");

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::ListItem { text, children, .. }
                if text.is_empty()
                    && matches!(
                        children.as_slice(),
                        [
                            MarkdownBlock::Heading { level: 1, text },
                            MarkdownBlock::Paragraph(details),
                        ] if text.plain_text() == "Heading"
                            && details.plain_text() == "details"
                    )
        ));
    }

    #[test]
    fn empty_link_first_list_paragraph_remains_a_child_block() {
        let document = MarkdownDocument::parse("- [](./target.md)\n\n  details");

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::ListItem { text, children, .. }
                if text.is_empty()
                    && matches!(
                        children.as_slice(),
                        [
                            MarkdownBlock::Paragraph(empty),
                            MarkdownBlock::Paragraph(details),
                        ] if empty.is_empty() && details.plain_text() == "details"
                    )
        ));
    }

    #[test]
    fn list_item_placeholder_is_only_for_truly_empty_items() {
        let mut text = RichText::default();
        text.push("content", InlineStyle::default());

        assert!(list_item_needs_empty_placeholder(
            &RichText::default(),
            false
        ));
        assert!(!list_item_needs_empty_placeholder(
            &RichText::default(),
            true
        ));
        assert!(!list_item_needs_empty_placeholder(&text, false));
    }

    #[test]
    fn html_blocks_store_raw_text_but_inline_html_is_hidden() {
        let document = MarkdownDocument::parse(
            "<section>\nraw\n</section>\n\nText <kbd>Esc</kbd>\n\nfoo<br />bar\n\nnot <bracket> tag",
        );

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::HtmlBlock { text } if text.contains("<section>") && text.contains("</section>")
        ));
        assert!(matches!(
            &document.blocks[1],
            MarkdownBlock::Paragraph(text) if text.plain_text() == "Text Esc"
        ));
        assert!(matches!(
            &document.blocks[2],
            MarkdownBlock::Paragraph(text) if text.plain_text() == "foo\nbar"
        ));
        assert!(matches!(
            &document.blocks[3],
            MarkdownBlock::Paragraph(text) if text.plain_text() == "not  tag"
        ));

        assert!(inline_html_is_line_break("<br>"));
        assert!(inline_html_is_line_break("<br/>"));
        assert!(inline_html_is_line_break("<BR class=\"line\">"));
        assert!(!inline_html_is_line_break("</br>"));
        assert!(!inline_html_is_line_break("<bracket>"));
    }

    #[test]
    fn html_block_wrappers_do_not_add_visible_placeholders() {
        let document = MarkdownDocument::parse("<div>\n\n*Emphasized* text.\n\n</div>");

        assert!(matches!(
            document.blocks.as_slice(),
            [
                MarkdownBlock::HtmlBlock { text: opening },
                MarkdownBlock::Paragraph(paragraph),
                MarkdownBlock::HtmlBlock { text: closing },
            ] if visible_html_text(opening).is_empty()
                && paragraph.plain_text() == "Emphasized text."
                && paragraph.spans().iter().any(|span| span.style.italic)
                && visible_html_text(closing).is_empty()
        ));
    }

    #[test]
    fn inline_html_tags_do_not_render_as_literal_text() {
        let document = MarkdownDocument::parse(
            "foo<br />bar\n\nfoo <BR class=\"line\"> baz\n\nnot <bracket> tag\n\n<del>*foo*</del>",
        );

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::Paragraph(text) if text.plain_text() == "foo\nbar"
        ));
        assert!(matches!(
            &document.blocks[1],
            MarkdownBlock::Paragraph(text) if text.plain_text() == "foo \n baz"
        ));
        assert!(matches!(
            &document.blocks[2],
            MarkdownBlock::Paragraph(text) if text.plain_text() == "not  tag"
        ));
        assert!(matches!(
            &document.blocks[3],
            MarkdownBlock::Paragraph(text)
                if text.plain_text() == "foo" && text.spans().iter().any(|span| span.style.italic)
        ));

        assert!(inline_html_is_line_break("<br>"));
        assert!(inline_html_is_line_break("<br/>"));
        assert!(inline_html_is_line_break("<BR class=\"line\">"));
        assert!(!inline_html_is_line_break("</br>"));
        assert!(!inline_html_is_line_break("<bracket>"));
    }

    #[test]
    fn commonmark_inline_del_preserves_nested_markdown_style() {
        let document = MarkdownDocument::parse("<del>*foo*</del>");

        assert!(matches!(
            document.blocks.as_slice(),
            [MarkdownBlock::Paragraph(text)]
                if text.plain_text() == "foo"
                    && text
                        .spans()
                        .iter()
                        .any(|span| span.style.italic && span.style.strikethrough)
        ));
    }

    #[test]
    fn inline_html_styles_do_not_leak_between_blocks() {
        let document = MarkdownDocument::parse("<em>foo\n\nbar\n\n- <strong>item\n- next");

        assert!(matches!(
            document.blocks.as_slice(),
            [
                MarkdownBlock::Paragraph(first),
                MarkdownBlock::Paragraph(second),
                MarkdownBlock::ListItem { text: item, .. },
                MarkdownBlock::ListItem { text: next, .. },
            ] if first.plain_text() == "foo"
                && first.spans().iter().any(|span| span.style.italic)
                && second.plain_text() == "bar"
                && second.spans().iter().all(|span| !span.style.italic)
                && item.plain_text() == "item"
                && item.spans().iter().any(|span| span.style.bold)
                && item.spans().iter().all(|span| !span.style.italic)
                && next.plain_text() == "next"
                && next
                    .spans()
                    .iter()
                    .all(|span| !span.style.bold && !span.style.italic)
        ));
    }

    #[test]
    fn inline_html_semantic_tags_update_text_style() {
        let document = MarkdownDocument::parse(
            "<strong>bold</strong> <em>em</em> <code>code</code> <kbd>key</kbd>",
        );
        let [MarkdownBlock::Paragraph(text)] = document.blocks.as_slice() else {
            panic!("expected one paragraph");
        };

        assert!(
            text.spans()
                .iter()
                .any(|span| span.text == "bold" && span.style.bold)
        );
        assert!(
            text.spans()
                .iter()
                .any(|span| span.text == "em" && span.style.italic)
        );
        assert!(
            text.spans()
                .iter()
                .any(|span| span.text == "code" && span.style.code)
        );
        assert!(
            text.spans()
                .iter()
                .any(|span| span.text == "key" && span.style.code)
        );
    }

    #[test]
    fn inline_html_anchors_expose_clickable_links() {
        let document = MarkdownDocument::parse(
            r#"<a href="/target?x=1&amp;y=2" title="A &lt; B">label</a> plain"#,
        );

        let [MarkdownBlock::Paragraph(text)] = document.blocks.as_slice() else {
            panic!("expected one paragraph");
        };

        assert_eq!(text.plain_text(), "label plain");
        assert_eq!(text.spans()[0].text, "label");
        assert_eq!(
            text.spans()[0].style.link_url.as_deref(),
            Some("/target?x=1&y=2")
        );
        assert_eq!(text.spans()[0].style.link_title.as_deref(), Some("A < B"));
        assert_eq!(text.spans()[1].text, " plain");
        assert!(text.spans()[1].style.link_url.is_none());
    }

    #[test]
    fn inline_html_anchor_links_do_not_leak_between_blocks() {
        let document = MarkdownDocument::parse(
            r#"<a href="/one">one

two"#,
        );

        assert!(matches!(
            document.blocks.as_slice(),
            [
                MarkdownBlock::Paragraph(one),
                MarkdownBlock::Paragraph(two),
            ] if one.plain_text() == "one"
                && one.spans().iter().all(|span| span.style.link_url.as_deref() == Some("/one"))
                && two.plain_text() == "two"
                && two.spans().iter().all(|span| span.style.link_url.is_none())
        ));
    }

    #[test]
    fn html_block_anchors_link_wrapped_markdown_blocks() {
        let document = MarkdownDocument::parse(
            r#"<a href="/target?x=1&amp;y=2" title="A &lt; B">

*label*

</a>

plain"#,
        );

        assert!(matches!(
            document.blocks.as_slice(),
            [
                MarkdownBlock::HtmlBlock { text: opening },
                MarkdownBlock::Paragraph(label),
                MarkdownBlock::HtmlBlock { text: closing },
                MarkdownBlock::Paragraph(plain),
            ] if visible_html_text(opening).is_empty()
                && label.plain_text() == "label"
                && label
                    .spans()
                    .iter()
                    .all(|span| span.style.italic
                        && span.style.link_url.as_deref() == Some("/target?x=1&y=2")
                        && span.style.link_title.as_deref() == Some("A < B"))
                && visible_html_text(closing).is_empty()
                && plain.plain_text() == "plain"
                && plain.spans().iter().all(|span| span.style.link_url.is_none())
        ));
    }

    #[test]
    fn html_block_style_tags_style_wrapped_markdown_blocks() {
        let document = MarkdownDocument::parse(
            "<del>\n\n*gone*\n\n</del>\n\n<strong>\n\nbold\n\n</strong>\n\nplain",
        );

        assert!(matches!(
            document.blocks.as_slice(),
            [
                MarkdownBlock::HtmlBlock { text: del_open },
                MarkdownBlock::Paragraph(gone),
                MarkdownBlock::HtmlBlock { text: del_close },
                MarkdownBlock::HtmlBlock { text: strong_open },
                MarkdownBlock::Paragraph(bold),
                MarkdownBlock::HtmlBlock { text: strong_close },
                MarkdownBlock::Paragraph(plain),
            ] if visible_html_text(del_open).is_empty()
                && gone.plain_text() == "gone"
                && gone
                    .spans()
                    .iter()
                    .all(|span| span.style.italic && span.style.strikethrough)
                && visible_html_text(del_close).is_empty()
                && visible_html_text(strong_open).is_empty()
                && bold.plain_text() == "bold"
                && bold.spans().iter().all(|span| span.style.bold)
                && visible_html_text(strong_close).is_empty()
                && plain.plain_text() == "plain"
                && plain
                    .spans()
                    .iter()
                    .all(|span| !span.style.bold && !span.style.strikethrough)
        ));
    }

    #[test]
    fn html_block_display_hides_markup() {
        assert_eq!(
            visible_html_text("<section>\nraw\n</section>\n").as_ref(),
            "raw"
        );
        assert_eq!(
            visible_html_text("<div id=\"foo\"\n  class=\"bar\">\ntext\n</div>").as_ref(),
            "text"
        );
        assert_eq!(visible_html_text("<!-- hidden -->").as_ref(), "");
        assert_eq!(
            visible_html_text("<!-- hidden -->\nvisible").as_ref(),
            "visible"
        );
        assert_eq!(visible_html_text("<![CDATA[a < b]]>").as_ref(), "a < b");
        assert_eq!(visible_html_text("").as_ref(), "");
    }

    #[test]
    fn html_block_display_decodes_visible_entities() {
        assert_eq!(
            visible_html_text("<div>&lt; &amp; &copy; &#35; &#x1F642;</div>").as_ref(),
            "< & © # 🙂"
        );
        assert_eq!(
            visible_html_text("<div>&notanentity; &amp without semicolon</div>").as_ref(),
            "&notanentity; &amp without semicolon"
        );
        assert_eq!(
            visible_html_text("<![CDATA[&lt; stays raw]]>").as_ref(),
            "&lt; stays raw"
        );
    }

    #[test]
    fn html_block_display_hides_unclosed_nontext_markup() {
        assert_eq!(visible_html_text("<!-- hidden\nstill hidden").as_ref(), "");
        assert_eq!(visible_html_text("<?php\necho 'hidden';").as_ref(), "");
        assert_eq!(visible_html_text("<!DOCTYPE html").as_ref(), "");
        assert_eq!(
            visible_html_text("<![CDATA[visible raw text").as_ref(),
            "visible raw text"
        );
    }

    #[test]
    fn html_block_display_hides_script_and_style_contents() {
        assert_eq!(
            visible_html_text("<script>\nalert('visible');\n</script>").as_ref(),
            ""
        );
        assert_eq!(
            visible_html_text("<style type=\"text/css\">\np { color: red; }\n</style>").as_ref(),
            ""
        );
        assert_eq!(
            visible_html_text("<pre><code>visible</code></pre>").as_ref(),
            "visible"
        );
    }

    #[test]
    fn html_block_display_ignores_quoted_greater_than_in_tags() {
        assert_eq!(
            visible_html_text("<a title=\">\">visible</a>").as_ref(),
            "visible"
        );
        assert_eq!(
            visible_html_text(
                "<a foo=\"bar\" bam = 'baz <em>\"</em>'\n_boolean zoop:33=zoop:33 />"
            )
            .as_ref(),
            ""
        );
    }

    #[test]
    fn html_block_display_preserves_invalid_tag_text() {
        assert_eq!(
            visible_html_text(
                "<div>\n<33> <__>\n<a h*#ref=\"hi\">\n<a href=\"hi'> <a href=hi'>\n</div>"
            )
            .as_ref(),
            "<33> <__>\n<a h*#ref=\"hi\">\n<a href=\"hi'> <a href=hi'>"
        );
    }

    #[test]
    fn commonmark_style_block_does_not_render_css_as_text() {
        let document = MarkdownDocument::parse("<style>p{color:red;}</style>\n*foo*");

        assert!(matches!(
            document.blocks.as_slice(),
            [
                MarkdownBlock::HtmlBlock { text },
                MarkdownBlock::Paragraph(paragraph),
            ] if visible_html_text(text).is_empty()
                && paragraph.plain_text() == "foo"
                && paragraph.spans().iter().any(|span| span.style.italic)
        ));
    }

    #[test]
    fn footnote_looking_text_is_not_dropped_as_an_extension() {
        let document = MarkdownDocument::parse("Footnote [^1].");

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::Paragraph(text) if text.plain_text() == "Footnote [^1]."
        ));
    }

    #[test]
    fn standalone_image_preserves_destination_and_plain_alt_text() {
        let document = MarkdownDocument::parse("![*logo*](https://example.com/logo.png)");

        let MarkdownBlock::Image {
            url,
            alt,
            title,
            link_url,
            link_title,
        } = &document.blocks[0]
        else {
            panic!("expected standalone image block");
        };

        assert_eq!(url.as_ref(), "https://example.com/logo.png");
        assert!(title.is_none());
        assert!(link_url.is_none());
        assert!(link_title.is_none());
        assert_eq!(alt.plain_text(), "logo");
        assert_eq!(alt.spans().len(), 1);
        let span = &alt.spans()[0];
        assert!(!span.style.italic);
        assert!(!span.style.link);
        assert_eq!(span.style.link_url.as_deref(), None);
    }

    #[test]
    fn standalone_image_keeps_empty_alt_text_empty() {
        let document = MarkdownDocument::parse("![](image.png)");

        let MarkdownBlock::Image {
            url,
            alt,
            title,
            link_url,
            link_title,
        } = &document.blocks[0]
        else {
            panic!("expected standalone image block");
        };

        assert_eq!(url.as_ref(), "image.png");
        assert!(title.is_none());
        assert!(link_url.is_none());
        assert!(link_title.is_none());
        assert!(alt.is_empty());
    }

    #[test]
    fn standalone_nested_image_uses_outer_destination() {
        let document = MarkdownDocument::parse("![foo ![bar](/bar.png)](/outer.png)");

        let MarkdownBlock::Image {
            url,
            alt,
            title,
            link_url,
            link_title,
        } = &document.blocks[0]
        else {
            panic!("expected standalone image block");
        };

        assert_eq!(url.as_ref(), "/outer.png");
        assert!(title.is_none());
        assert!(link_url.is_none());
        assert!(link_title.is_none());
        assert_eq!(alt.plain_text(), "foo bar");
    }

    #[test]
    fn list_item_standalone_image_becomes_child_block() {
        let document = MarkdownDocument::parse("- ![logo](logo.png)");

        let MarkdownBlock::ListItem { text, children, .. } = &document.blocks[0] else {
            panic!("expected list item");
        };

        assert!(text.is_empty());
        assert!(matches!(
            children.as_slice(),
            [MarkdownBlock::Image { url, alt, title, link_url, link_title }]
                if url.as_ref() == "logo.png"
                    && alt.plain_text() == "logo"
                    && title.is_none()
                    && link_url.is_none()
                    && link_title.is_none()
        ));
    }

    #[test]
    fn standalone_image_preserves_title() {
        let document = MarkdownDocument::parse("![logo](logo.png \"Logo title\")");

        let MarkdownBlock::Image { url, title, .. } = &document.blocks[0] else {
            panic!("expected standalone image block");
        };

        assert_eq!(url.as_ref(), "logo.png");
        assert_eq!(title.as_deref(), Some("Logo title"));
    }

    #[test]
    fn inline_image_alt_text_is_retained_as_image_span() {
        let document = MarkdownDocument::parse("See ![logo](https://example.com/logo.png).");

        let MarkdownBlock::Paragraph(text) = &document.blocks[0] else {
            panic!("expected inline image paragraph");
        };

        assert_eq!(text.plain_text(), "See logo.");
        assert_eq!(text.inline_images().len(), 1);
        let image = &text.inline_images()[0];
        assert_eq!(image.range, 4..8);
        assert_eq!(image.url.as_ref(), "https://example.com/logo.png");
        assert!(image.title.is_none());
        assert!(image.link_url.is_none());
        assert_eq!(image.alt.plain_text(), "logo");
        let alt_span = &image.alt.spans()[0];
        assert!(!alt_span.style.italic);
        assert!(!alt_span.style.link);
        assert_eq!(alt_span.style.link_url.as_deref(), None);
    }

    #[test]
    fn inline_empty_image_alt_text_uses_source_as_fallback_span() {
        let document = MarkdownDocument::parse("See ![](image.png)");

        let MarkdownBlock::Paragraph(text) = &document.blocks[0] else {
            panic!("expected inline image paragraph");
        };

        assert_eq!(text.plain_text(), "See image.png");
        assert_eq!(text.inline_images().len(), 1);
        let image = &text.inline_images()[0];
        assert_eq!(image.range, 4..13);
        assert_eq!(image.url.as_ref(), "image.png");
        assert!(image.title.is_none());
        assert!(image.alt.is_empty());
        assert!(image.link_url.is_none());
    }

    #[test]
    fn inline_nested_image_uses_outer_destination() {
        let document = MarkdownDocument::parse("See ![foo ![bar](/bar.png)](/outer.png).");

        let MarkdownBlock::Paragraph(text) = &document.blocks[0] else {
            panic!("expected inline image paragraph");
        };

        assert_eq!(text.plain_text(), "See foo bar.");
        assert_eq!(text.inline_images().len(), 1);
        let image = &text.inline_images()[0];
        assert_eq!(image.url.as_ref(), "/outer.png");
        assert!(image.title.is_none());
        assert_eq!(image.alt.plain_text(), "foo bar");
    }

    #[test]
    fn inline_image_preserves_title() {
        let document = MarkdownDocument::parse("See ![logo](logo.png \"Logo title\").");

        let MarkdownBlock::Paragraph(text) = &document.blocks[0] else {
            panic!("expected inline image paragraph");
        };

        assert_eq!(text.inline_images().len(), 1);
        let image = &text.inline_images()[0];
        assert_eq!(image.url.as_ref(), "logo.png");
        assert_eq!(image.title.as_deref(), Some("Logo title"));
    }

    #[test]
    fn standalone_image_inside_link_preserves_outer_link_destination() {
        let document = MarkdownDocument::parse("[![logo](logo.png)](https://example.com)");

        let MarkdownBlock::Image {
            url,
            alt,
            title,
            link_url,
            link_title,
        } = &document.blocks[0]
        else {
            panic!("expected linked image block");
        };

        assert_eq!(url.as_ref(), "logo.png");
        assert!(title.is_none());
        assert_eq!(alt.plain_text(), "logo");
        assert_eq!(link_url.as_deref(), Some("https://example.com"));
        assert!(link_title.is_none());
    }

    #[test]
    fn standalone_image_inside_link_preserves_outer_link_title() {
        let document =
            MarkdownDocument::parse("[![logo](logo.png)](https://example.com \"Example title\")");

        let MarkdownBlock::Image {
            url,
            alt,
            title,
            link_url,
            link_title,
        } = &document.blocks[0]
        else {
            panic!("expected linked image block");
        };

        assert_eq!(url.as_ref(), "logo.png");
        assert_eq!(alt.plain_text(), "logo");
        assert!(title.is_none());
        assert_eq!(link_url.as_deref(), Some("https://example.com"));
        assert_eq!(link_title.as_deref(), Some("Example title"));
    }

    #[test]
    fn inline_image_inside_link_uses_outer_link_destination() {
        let document = MarkdownDocument::parse("[![logo](logo.png)](https://example.com) now");

        let MarkdownBlock::Paragraph(text) = &document.blocks[0] else {
            panic!("expected linked image fallback paragraph");
        };

        assert_eq!(text.plain_text(), "logo now");
        assert_eq!(text.inline_images().len(), 1);
        let image = &text.inline_images()[0];
        assert_eq!(image.url.as_ref(), "logo.png");
        assert!(image.title.is_none());
        assert_eq!(image.alt.plain_text(), "logo");
        assert_eq!(image.link_url.as_deref(), Some("https://example.com"));
        assert!(image.link_title.is_none());
    }

    #[test]
    fn inline_image_inside_link_preserves_outer_link_title() {
        let document =
            MarkdownDocument::parse("[![logo](logo.png)](https://example.com \"Example title\")");

        let MarkdownBlock::Image { link_title, .. } = &document.blocks[0] else {
            panic!("expected linked image block");
        };

        assert_eq!(link_title.as_deref(), Some("Example title"));

        let document = MarkdownDocument::parse(
            "[![logo](logo.png)](https://example.com \"Example title\") now",
        );
        let MarkdownBlock::Paragraph(text) = &document.blocks[0] else {
            panic!("expected linked image paragraph");
        };

        assert_eq!(text.inline_images().len(), 1);
        let image = &text.inline_images()[0];
        assert_eq!(image.link_url.as_deref(), Some("https://example.com"));
        assert_eq!(image.link_title.as_deref(), Some("Example title"));
    }

    #[test]
    fn empty_linked_image_fallback_uses_outer_link_destination() {
        let fallback = image_fallback_text(
            &SharedString::from("logo.png"),
            RichText::default(),
            Some(SharedString::from("https://example.com")),
            Some(SharedString::from("Example title")),
        );

        assert_eq!(fallback.plain_text(), "logo.png");
        assert_eq!(fallback.spans().len(), 1);
        assert_eq!(
            fallback.spans()[0].style.link_url.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            fallback.spans()[0].style.link_title.as_deref(),
            Some("Example title")
        );
    }

    #[test]
    fn nonempty_linked_image_fallback_uses_outer_link_metadata() {
        let fallback = image_fallback_text(
            &SharedString::from("logo.png"),
            rich_text_from_plain("logo"),
            Some(SharedString::from("https://example.com")),
            Some(SharedString::from("Example title")),
        );

        assert_eq!(fallback.plain_text(), "logo");
        assert_eq!(fallback.spans().len(), 1);
        assert_eq!(
            fallback.spans()[0].style.link_url.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            fallback.spans()[0].style.link_title.as_deref(),
            Some("Example title")
        );
    }

    #[test]
    fn empty_fenced_code_blocks_are_kept_renderable() {
        let document = MarkdownDocument::parse("```\n```");

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::CodeBlock { language: None, text } if text.is_empty()
        ));
        assert_eq!(visible_code_text(""), SharedString::from(" "));
    }

    #[test]
    fn code_block_display_text_preserves_meaningful_blank_lines() {
        assert_eq!(code_block_display_text("code\n"), "code");
        assert_eq!(code_block_display_text("code\n\n"), "code\n");
        assert_eq!(code_block_display_text("\n"), "");
        assert_eq!(
            visible_code_text(&code_block_display_text("\n")),
            SharedString::from(" ")
        );
    }

    #[test]
    fn fenced_code_info_strings_decode_escapes_and_entities() {
        for (source, expected_language) in [
            ("``` foo\\+bar\nfoo\n```", "foo+bar"),
            ("``` f&ouml;&ouml;\nfoo\n```", "föö"),
            ("~~~~    ruby startline=3 $%@#$\nfoo\n~~~~~~~", "ruby"),
        ] {
            let document = MarkdownDocument::parse(source);

            assert!(matches!(
                document.blocks.as_slice(),
                [MarkdownBlock::CodeBlock { language: Some(language), text }]
                    if language == expected_language && text == "foo\n"
            ));
        }
    }

    #[test]
    fn two_backtick_lines_parse_as_an_inline_code_span() {
        let document = MarkdownDocument::parse("``\nfoo\n``");

        assert_eq!(document.blocks.len(), 1);
        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::Paragraph(text)
                if text.plain_text() == "foo"
                    && text.spans().iter().any(|span| span.style.code)
        ));
    }

    #[test]
    fn unmatched_two_backticks_stay_literal_text() {
        let document = MarkdownDocument::parse("``");

        assert!(matches!(
            document.blocks.as_slice(),
            [MarkdownBlock::Paragraph(text)]
                if text.plain_text() == "``"
                    && text.spans().iter().all(|span| !span.style.code)
        ));
    }

    #[test]
    fn empty_two_backtick_code_span_keeps_visible_space() {
        for source in ["`` ``", "``\n``"] {
            let document = MarkdownDocument::parse(source);

            assert!(matches!(
                document.blocks.as_slice(),
                [MarkdownBlock::Paragraph(text)]
                    if text.plain_text() == " "
                        && text.spans().iter().any(|span| span.style.code)
            ));
        }
    }

    #[test]
    fn thematic_breaks_inside_list_items_remain_nested() {
        let document = MarkdownDocument::parse("- item\n  * * *");

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::ListItem { text, children, .. }
                if text.plain_text() == "item"
                    && matches!(children.as_slice(), [MarkdownBlock::Rule])
        ));
    }

    #[test]
    fn empty_list_items_are_preserved() {
        let document = MarkdownDocument::parse("* a\n*\n* b");

        assert_eq!(document.blocks.len(), 3);
        assert!(matches!(
            &document.blocks[1],
            MarkdownBlock::ListItem { text, children, .. }
                if text.is_empty() && children.is_empty()
        ));
    }

    #[test]
    fn empty_block_quotes_are_preserved() {
        let document = MarkdownDocument::parse(">");

        assert!(matches!(
            &document.blocks[0],
            MarkdownBlock::BlockQuote { blocks, .. } if blocks.is_empty()
        ));
    }

    #[test]
    fn preview_style_uses_document_typography_and_scrolling() {
        let tokens = DesignTokens::dark();
        let default = MarkdownStyle::from_tokens(&tokens);
        let preview = MarkdownStyle::preview_from_tokens(&tokens);

        assert!(!default.preview);
        assert!(preview.preview);
        assert!(preview.body_font_size > default.body_font_size);
        assert!(preview.heading_font_sizes[0] > preview.heading_font_sizes[1]);
        assert!(preview.heading_border_color.is_some());
        assert!(preview.code_overflow_x_scroll);
    }

    #[test]
    fn rich_text_exposes_clickable_link_ranges() {
        let document = MarkdownDocument::parse("See [docs](https://example.com) now.");
        let MarkdownBlock::Paragraph(text) = document.blocks[0].clone() else {
            panic!("expected paragraph");
        };
        let tokens = DesignTokens::dark();
        let style = MarkdownStyle::from_tokens(&tokens);
        let parts = text.into_render_parts(&style);
        let plain = parts.text.to_string();

        assert_eq!(plain, "See docs now.");
        assert_eq!(parts.links.len(), 1);
        assert_eq!(&plain[parts.links[0].range.clone()], "docs");
        assert_eq!(parts.links[0].url.as_ref(), "https://example.com");
        assert!(parts.links[0].title.is_none());
    }

    #[test]
    fn rich_text_exposes_link_titles() {
        let document = MarkdownDocument::parse("See [docs](https://example.com \"Docs title\").");
        let MarkdownBlock::Paragraph(text) = document.blocks[0].clone() else {
            panic!("expected paragraph");
        };
        let tokens = DesignTokens::dark();
        let style = MarkdownStyle::from_tokens(&tokens);

        assert_eq!(text.spans().len(), 3);
        assert_eq!(text.spans()[1].text, "docs");
        assert_eq!(
            text.spans()[1].style.link_url.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            text.spans()[1].style.link_title.as_deref(),
            Some("Docs title")
        );

        let parts = text.into_render_parts(&style);
        let plain = parts.text.to_string();

        assert_eq!(parts.links.len(), 1);
        assert_eq!(&plain[parts.links[0].range.clone()], "docs");
        assert_eq!(parts.links[0].url.as_ref(), "https://example.com");
        assert_eq!(parts.links[0].title.as_deref(), Some("Docs title"));
    }

    #[test]
    fn link_destinations_use_commonmark_url_escaping() {
        for (source, expected_url) in [
            ("[foo](/f&ouml;&ouml; \"f&ouml;&ouml;\")", "/f%C3%B6%C3%B6"),
            ("[foo]\n\n[foo]: <my url>", "my%20url"),
            (
                "[foo]\n\n[foo]: /url\\bar\\*baz \"title\"",
                "/url%5Cbar*baz",
            ),
            (
                "[foo](https://example.com?foo=3&bar=4)",
                "https://example.com?foo=3&bar=4",
            ),
        ] {
            let document = MarkdownDocument::parse(source);
            let MarkdownBlock::Paragraph(text) = &document.blocks[0] else {
                panic!("expected linked paragraph");
            };

            assert_eq!(
                text.spans()[0].style.link_url.as_deref(),
                Some(expected_url)
            );
        }
    }

    #[test]
    fn image_destinations_use_commonmark_url_escaping() {
        let document = MarkdownDocument::parse("![foo](/f&ouml;&ouml; \"f&ouml;&ouml;\")");

        let MarkdownBlock::Image { url, title, .. } = &document.blocks[0] else {
            panic!("expected image block");
        };

        assert_eq!(url.as_ref(), "/f%C3%B6%C3%B6");
        assert_eq!(title.as_deref(), Some("föö"));
    }

    #[test]
    fn autolinks_expose_commonmark_link_destinations() {
        let document = MarkdownDocument::parse("<https://example.com>\n\n<foo@bar.example.com>");

        let MarkdownBlock::Paragraph(text) = document.blocks[0].clone() else {
            panic!("expected URL autolink paragraph");
        };
        assert_eq!(text.plain_text(), "https://example.com");
        assert_eq!(
            text.spans()[0].style.link_url.as_deref(),
            Some("https://example.com")
        );

        let MarkdownBlock::Paragraph(text) = document.blocks[1].clone() else {
            panic!("expected email autolink paragraph");
        };
        assert_eq!(text.plain_text(), "foo@bar.example.com");
        assert_eq!(
            text.spans()[0].style.link_url.as_deref(),
            Some("mailto:foo@bar.example.com")
        );
    }

    #[gpui::test]
    fn markdown_link_click_opens_url(cx: &mut gpui::TestAppContext) {
        use gpui::{Context, Modifiers, Render, point};

        struct MarkdownLinkFixture {
            style: MarkdownStyle,
        }

        impl Render for MarkdownLinkFixture {
            fn render(
                &mut self,
                _window: &mut gpui::Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                div()
                    .w(px(240.0))
                    .h(px(48.0))
                    .child(markdown("[docs](https://example.com)", self.style.clone()))
            }
        }

        let tokens = DesignTokens::dark();
        let style = MarkdownStyle::from_tokens(&tokens).compact();

        let (_view, cx) = cx.add_window_view(move |_window, _cx| MarkdownLinkFixture { style });
        cx.run_until_parked();
        cx.simulate_click(point(px(12.0), px(12.0)), Modifiers::default());

        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com"));
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

    #[test]
    fn code_language_normalization_accepts_fence_info_strings() {
        assert_eq!(
            normalized_code_language(Some("{rust,ignore}")),
            Some("rust".to_string())
        );
        assert_eq!(
            normalized_code_language(Some("sql title=\"query\"")),
            Some("sql".to_string())
        );
        assert_eq!(normalized_code_language(Some("   ")), None);
        assert_eq!(normalized_code_language(None), None);
    }

    #[test]
    fn code_syntax_highlights_falls_back_without_theme_or_loader() {
        assert!(
            code_syntax_highlights("fn main() {}", Some("rust"), None, None).is_empty(),
            "missing theme/loader should render code as plain text"
        );
    }

    #[test]
    fn helix_style_to_highlight_style_maps_text_attributes() {
        use helix_view::graphics::Color;

        let style = HelixStyle::default()
            .fg(Color::Rgb(0x12, 0x34, 0x56))
            .bg(Color::Rgb(0x22, 0x33, 0x44))
            .underline_color(Color::Rgb(0xaa, 0xbb, 0xcc))
            .underline_style(HelixUnderlineStyle::Curl)
            .add_modifier(HelixModifier::BOLD | HelixModifier::ITALIC | HelixModifier::CROSSED_OUT);
        let highlight = helix_style_to_highlight_style(style).expect("style should map");

        assert_eq!(highlight.color, color_to_hsla(Color::Rgb(0x12, 0x34, 0x56)));
        assert_eq!(
            highlight.background_color,
            color_to_hsla(Color::Rgb(0x22, 0x33, 0x44))
        );
        assert_eq!(highlight.font_weight, Some(FontWeight::BOLD));
        assert_eq!(highlight.font_style, Some(FontStyle::Italic));
        assert!(highlight.underline.is_some_and(|underline| underline.wavy));
        assert!(highlight.strikethrough.is_some());
    }

    #[gpui::test]
    fn markdown_reports_overflow_in_scroll_container(cx: &mut gpui::TestAppContext) {
        use gpui::{
            Context, InteractiveElement, Render, ScrollDelta, ScrollWheelEvent,
            StatefulInteractiveElement, TouchPhase, point,
        };

        struct MarkdownScrollFixture {
            scroll: gpui::ScrollHandle,
            source: SharedString,
            style: MarkdownStyle,
        }

        impl Render for MarkdownScrollFixture {
            fn render(
                &mut self,
                _window: &mut gpui::Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                div().w(px(380.0)).h(px(120.0)).child(
                    div()
                        .id("markdown-scroll-regression")
                        .w(px(380.0))
                        .h(px(120.0))
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(markdown(self.source.clone(), self.style.clone())),
                )
            }
        }

        let source = (0..32)
            .map(|index| format!("Paragraph {index}\n\nSome hover documentation text."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let scroll = gpui::ScrollHandle::new();
        let tokens = DesignTokens::dark();
        let style = MarkdownStyle::from_tokens(&tokens).compact();

        let (_view, cx) = cx.add_window_view({
            let scroll = scroll.clone();
            let source = SharedString::from(source);
            move |_window, _cx| MarkdownScrollFixture {
                scroll,
                source,
                style,
            }
        });
        cx.run_until_parked();

        assert!(
            scroll.max_offset().y > px(0.0),
            "expected markdown content to overflow the scroll viewport, got {:?}",
            scroll.max_offset()
        );

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(20.0), px(20.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-48.0))),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        });

        assert!(
            scroll.offset().y < px(0.0),
            "expected wheel input to move markdown scroll offset, got {:?}",
            scroll.offset()
        );
    }
}
