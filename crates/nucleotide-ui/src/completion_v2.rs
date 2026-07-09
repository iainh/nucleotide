// ABOUTME: Enhanced completion system with async filtering and smart query optimization
// ABOUTME: Professional-grade completion view based on Zed's architecture

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Task, Window, div, px, relative,
};
use std::cmp::Ordering as CmpOrdering;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::completion_cache::{CacheKey, CompletionCache};
use crate::completion_docs::{
    DocumentationCacheConfig, DocumentationContent, DocumentationLoader, DocumentationPanel,
};
use crate::completion_error::{
    CompletionError, CompletionErrorHandler, ErrorContext, ErrorHandlingResult,
};
use crate::completion_perf::{PerformanceMonitor, PerformanceTimer};
use crate::completion_renderer::{CompletionItemElement, CompletionListState};
use crate::debouncer::{CompletionDebouncer, create_completion_debouncer};
// use crate::fuzzy::{FuzzyConfig, match_strings}; // Unused in synchronous filtering
use crate::actions::completion::{
    CompletionConfirm, CompletionConfirmAndStop, CompletionDismiss, CompletionPageDown,
    CompletionPageUp, CompletionSelectFirst, CompletionSelectLast, CompletionSelectNext,
    CompletionSelectPrev,
};

pub(crate) const COMPLETION_CONTEXT: &str = "Completion";
const COMPLETION_VISIBLE_ROWS: usize = 6;
const COMPLETION_ROW_HEIGHT_PX: f32 = 32.0;
const COMPLETION_LIST_MAX_HEIGHT_PX: f32 =
    COMPLETION_VISIBLE_ROWS as f32 * COMPLETION_ROW_HEIGHT_PX;

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", CompletionSelectPrev, Some(COMPLETION_CONTEXT)),
        KeyBinding::new("down", CompletionSelectNext, Some(COMPLETION_CONTEXT)),
        KeyBinding::new("ctrl-p", CompletionSelectPrev, Some(COMPLETION_CONTEXT)),
        KeyBinding::new("ctrl-n", CompletionSelectNext, Some(COMPLETION_CONTEXT)),
        KeyBinding::new("enter", CompletionConfirm, Some(COMPLETION_CONTEXT)),
        KeyBinding::new("tab", CompletionConfirmAndStop, Some(COMPLETION_CONTEXT)),
        KeyBinding::new("escape", CompletionDismiss, Some(COMPLETION_CONTEXT)),
        KeyBinding::new("ctrl-g", CompletionDismiss, Some(COMPLETION_CONTEXT)),
        KeyBinding::new("home", CompletionSelectFirst, Some(COMPLETION_CONTEXT)),
        KeyBinding::new("end", CompletionSelectLast, Some(COMPLETION_CONTEXT)),
        KeyBinding::new("pageup", CompletionPageUp, Some(COMPLETION_CONTEXT)),
        KeyBinding::new("pagedown", CompletionPageDown, Some(COMPLETION_CONTEXT)),
    ]);
}

/// Logical menu operations used by focused completion views and editor bridges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionMenuAction {
    Confirm,
    Dismiss,
    SelectNext,
    SelectPrevious,
}

/// Maps Helix-style completion navigation keys to logical completion actions.
///
/// This keeps the editor bridge from duplicating completion menu semantics while
/// still allowing the editor to keep focus during completion.
pub fn completion_menu_action_for_key(
    key: &str,
    control: bool,
    shift: bool,
) -> Option<CompletionMenuAction> {
    match (key, control, shift) {
        ("escape", false, false) => Some(CompletionMenuAction::Dismiss),
        ("tab", false, false) | ("y", true, false) => Some(CompletionMenuAction::Confirm),
        ("down", false, false) | ("n", true, false) => Some(CompletionMenuAction::SelectNext),
        ("up", false, false) | ("p", true, false) => Some(CompletionMenuAction::SelectPrevious),
        _ => None,
    }
}

/// Event emitted to request completion acceptance via Helix's Transaction system
/// This event signals that Helix should handle the completion acceptance
#[derive(Debug, Clone)]
pub struct CompleteViaHelixEvent {
    pub item_index: usize,
}

/// Event emitted when completion has a non-fatal user-facing warning.
#[derive(Debug, Clone)]
pub struct CompletionWarningEvent {
    pub message: SharedString,
}

/// Candidate for fuzzy matching - lightweight representation of completion items
#[derive(Debug, Clone)]
pub struct StringMatchCandidate {
    /// Unique identifier for this candidate
    pub id: usize,
    /// Index of the source item in the completion list
    pub item_index: usize,
    /// Text content to match against
    pub text: String,
    /// Lowercased text used by the completion filter hot path
    normalized_text: String,
}

impl StringMatchCandidate {
    pub fn new(id: usize, text: String) -> Self {
        Self::with_index(id, text, 0)
    }

    pub fn with_index(id: usize, text: String, item_index: usize) -> Self {
        let normalized_text = text.to_lowercase();
        Self {
            id,
            item_index,
            text,
            normalized_text,
        }
    }

    fn from_item(index: usize, item: &CompletionItem) -> Self {
        let mut candidate = Self::from(item);
        candidate.item_index = index;
        candidate
    }
}

fn fuzzy_match_candidate(
    candidate: &StringMatchCandidate,
    query_lower: &str,
) -> Option<StringMatch> {
    if query_lower.is_empty() {
        return Some(StringMatch::for_candidate(candidate, 100, Vec::new()));
    }

    if candidate.normalized_text == query_lower {
        return Some(StringMatch::for_candidate(
            candidate,
            1000,
            contiguous_match_positions(&candidate.normalized_text, 0, query_lower),
        ));
    }

    if candidate.normalized_text.starts_with(query_lower) {
        let score = 900u16.saturating_sub(length_penalty(&candidate.normalized_text, query_lower));
        return Some(StringMatch::for_candidate(
            candidate,
            score,
            contiguous_match_positions(&candidate.normalized_text, 0, query_lower),
        ));
    }

    if let Some(byte_start) = candidate.normalized_text.find(query_lower) {
        let start = candidate.normalized_text[..byte_start].chars().count();
        let boundary_bonus = word_boundary_bonus(&candidate.text, start);
        let score = 700u16
            .saturating_add(boundary_bonus)
            .saturating_sub(start.min(80) as u16)
            .saturating_sub(length_penalty(&candidate.normalized_text, query_lower));
        return Some(StringMatch::for_candidate(
            candidate,
            score,
            contiguous_match_positions(&candidate.normalized_text, start, query_lower),
        ));
    }

    subsequence_match(candidate, query_lower)
}

fn subsequence_match(candidate: &StringMatchCandidate, query_lower: &str) -> Option<StringMatch> {
    let mut positions = Vec::with_capacity(query_lower.chars().count());
    let mut query_chars = query_lower.chars();
    let mut next_query = query_chars.next()?;
    let mut last_position = None;
    let mut consecutive_bonus = 0u16;
    let mut boundary_bonus = 0u16;

    for (position, ch) in candidate.normalized_text.chars().enumerate() {
        if ch != next_query {
            continue;
        }

        if last_position.is_some_and(|last| last + 1 == position) {
            consecutive_bonus = consecutive_bonus.saturating_add(12);
        }
        boundary_bonus =
            boundary_bonus.saturating_add(word_boundary_bonus(&candidate.text, position));
        positions.push(position);
        last_position = Some(position);

        match query_chars.next() {
            Some(next) => next_query = next,
            None => {
                let start_penalty = positions.first().copied().unwrap_or_default().min(80) as u16;
                let gap_penalty = positions
                    .windows(2)
                    .map(|window| window[1].saturating_sub(window[0] + 1).min(20) as u16)
                    .sum::<u16>();
                let score = 500u16
                    .saturating_add(consecutive_bonus)
                    .saturating_add(boundary_bonus)
                    .saturating_sub(start_penalty)
                    .saturating_sub(gap_penalty)
                    .saturating_sub(length_penalty(&candidate.normalized_text, query_lower));
                return Some(StringMatch::for_candidate(candidate, score, positions));
            }
        }
    }

    None
}

fn contiguous_match_positions(text: &str, start: usize, query: &str) -> Vec<usize> {
    let end = start + query.chars().count();
    let text_len = text.chars().count();
    (start..end.min(text_len)).collect()
}

fn query_prefix_boundaries_desc(query: &str) -> impl Iterator<Item = usize> + '_ {
    query
        .char_indices()
        .rev()
        .filter_map(|(index, _)| (index > 0).then_some(index))
}

fn length_penalty(text: &str, query: &str) -> u16 {
    text.chars()
        .count()
        .saturating_sub(query.chars().count())
        .min(80) as u16
}

fn word_boundary_bonus(text: &str, position: usize) -> u16 {
    if position == 0 {
        return 40;
    }

    let mut chars = text.chars();
    let previous = chars.nth(position.saturating_sub(1));
    let current = chars.next();

    match (previous, current) {
        (Some(prev), Some(curr))
            if prev == '_'
                || prev == '-'
                || prev == ':'
                || prev == '.'
                || (prev.is_lowercase() && curr.is_uppercase()) =>
        {
            30
        }
        _ => 0,
    }
}

impl From<&CompletionItem> for StringMatchCandidate {
    fn from(item: &CompletionItem) -> Self {
        let text = item.match_text().to_string();

        // For now, use text hash as id - in real implementation,
        // this would be an actual unique identifier
        let id = text.as_bytes().iter().fold(0usize, |acc, &b| {
            acc.wrapping_mul(31).wrapping_add(b as usize)
        });

        Self::new(id, text)
    }
}

/// Result of fuzzy matching with score and match positions
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringMatch {
    /// ID of the matched candidate
    pub candidate_id: usize,
    /// Index of the matched item in the source completion list
    pub item_index: usize,
    /// Match score (higher is better)
    pub score: u16,
    /// Character positions that matched in the original string
    pub positions: Vec<usize>,
}

impl StringMatch {
    pub fn new(candidate_id: usize, score: u16, positions: Vec<usize>) -> Self {
        Self {
            candidate_id,
            item_index: candidate_id,
            score,
            positions,
        }
    }

    fn for_candidate(candidate: &StringMatchCandidate, score: u16, positions: Vec<usize>) -> Self {
        Self {
            candidate_id: candidate.id,
            item_index: candidate.item_index,
            score,
            positions,
        }
    }
}

impl PartialOrd for StringMatch {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StringMatch {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Higher scores first
        other.score.cmp(&self.score)
    }
}

/// Enhanced completion item with richer metadata
#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub text: SharedString,
    pub description: Option<SharedString>,
    pub display_text: Option<SharedString>,
    /// Kind of completion (function, variable, etc.)
    pub kind: Option<CompletionItemKind>,
    /// Detailed documentation
    pub documentation: Option<SharedString>,
    /// Additional details like function signature or type information
    pub detail: Option<SharedString>,
    /// Function signature details (parameters, return type)
    pub signature_info: Option<SharedString>,
    /// Return type or module information
    pub type_info: Option<SharedString>,
    /// Insert text format (plain text or snippet)
    pub insert_text_format: InsertTextFormat,
    /// Optional edit metadata used to apply LSP completion edits precisely.
    pub edit: Option<CompletionEdit>,
    /// Server-provided sort key used to preserve language-server ranking intent.
    pub sort_text: Option<SharedString>,
    /// Server-provided match key. This is used for filtering instead of display text.
    pub filter_text: Option<SharedString>,
    /// Whether the server recommends this item as the initial selection.
    pub preselect: bool,
    pub commit_characters: Vec<SharedString>,
    pub tags: Vec<CompletionItemTag>,
    pub data: Option<serde_json::Value>,
    pub source_index: usize,
    pub selection_priority: u64,
    pub server_id: Option<u64>,
    pub raw_lsp_item: Option<serde_json::Value>,
    pub locality_score: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionItemTag {
    Deprecated,
}

fn compare_completion_items(left: &CompletionItem, right: &CompletionItem) -> CmpOrdering {
    compare_completion_text(left.sort_key(), right.sort_key())
        .then_with(|| {
            compare_completion_text(left.label_text().as_ref(), right.label_text().as_ref())
        })
        .then_with(|| completion_kind_rank(left.kind).cmp(&completion_kind_rank(right.kind)))
        .then_with(|| left.source_index.cmp(&right.source_index))
}

fn compare_completion_text(left: &str, right: &str) -> CmpOrdering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

fn completion_kind_rank(kind: Option<CompletionItemKind>) -> u8 {
    match kind {
        Some(CompletionItemKind::Method) => 0,
        Some(CompletionItemKind::Function) => 1,
        Some(CompletionItemKind::Field) | Some(CompletionItemKind::Property) => 2,
        Some(CompletionItemKind::Variable) => 3,
        Some(CompletionItemKind::Constructor) => 4,
        Some(CompletionItemKind::Class)
        | Some(CompletionItemKind::Interface)
        | Some(CompletionItemKind::Struct)
        | Some(CompletionItemKind::Enum)
        | Some(CompletionItemKind::TypeParameter) => 5,
        Some(CompletionItemKind::Module) => 6,
        Some(CompletionItemKind::Keyword) => 7,
        Some(CompletionItemKind::Snippet) => 8,
        Some(CompletionItemKind::Text) | None => 9,
        Some(CompletionItemKind::Unit) => 10,
        Some(CompletionItemKind::Value) => 11,
        Some(CompletionItemKind::Color) => 12,
        Some(CompletionItemKind::File) => 13,
        Some(CompletionItemKind::Reference) => 14,
        Some(CompletionItemKind::Folder) => 15,
        Some(CompletionItemKind::EnumMember) => 16,
        Some(CompletionItemKind::Constant) => 17,
        Some(CompletionItemKind::Event) => 18,
        Some(CompletionItemKind::Operator) => 19,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionEdit {
    pub offset_encoding: CompletionOffsetEncoding,
    pub text_edit: Option<CompletionTextEdit>,
    pub additional_text_edits: Vec<CompletionTextEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionTextEdit {
    pub range: CompletionRange,
    pub new_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionRange {
    pub start: CompletionPosition,
    pub end: CompletionPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionOffsetEncoding {
    Utf8,
    Utf16,
    Utf32,
}

/// LSP Insert Text Format
#[derive(Debug, Clone, PartialEq)]
pub enum InsertTextFormat {
    /// Plain text insertion
    PlainText,
    /// Snippet with tabstops and placeholders ($0, $1, ${1:placeholder})
    Snippet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionItemKind {
    Text,
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
}

impl CompletionItem {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            description: None,
            display_text: None,
            kind: None,
            documentation: None,
            detail: None,
            signature_info: None,
            type_info: None,
            insert_text_format: InsertTextFormat::PlainText,
            edit: None,
            sort_text: None,
            filter_text: None,
            preselect: false,
            commit_characters: Vec::new(),
            tags: Vec::new(),
            data: None,
            source_index: 0,
            selection_priority: 0,
            server_id: None,
            raw_lsp_item: None,
            locality_score: 0,
        }
    }

    fn label_text(&self) -> &SharedString {
        self.display_text.as_ref().unwrap_or(&self.text)
    }

    fn match_text(&self) -> &SharedString {
        self.filter_text
            .as_ref()
            .unwrap_or_else(|| self.label_text())
    }

    fn sort_key(&self) -> &str {
        self.sort_text
            .as_ref()
            .map(|text| text.as_ref())
            .unwrap_or_else(|| self.label_text().as_ref())
    }

    pub fn with_description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_display_text(mut self, display_text: impl Into<SharedString>) -> Self {
        self.display_text = Some(display_text.into());
        self
    }

    pub fn with_kind(mut self, kind: CompletionItemKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn with_documentation(mut self, documentation: impl Into<SharedString>) -> Self {
        self.documentation = Some(documentation.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_signature_info(mut self, signature_info: impl Into<SharedString>) -> Self {
        self.signature_info = Some(signature_info.into());
        self
    }

    pub fn with_type_info(mut self, type_info: impl Into<SharedString>) -> Self {
        self.type_info = Some(type_info.into());
        self
    }

    pub fn with_insert_text_format(mut self, format: InsertTextFormat) -> Self {
        self.insert_text_format = format;
        self
    }

    pub fn with_edit(mut self, edit: CompletionEdit) -> Self {
        self.edit = Some(edit);
        self
    }

    pub fn with_sort_text(mut self, sort_text: impl Into<SharedString>) -> Self {
        self.sort_text = Some(sort_text.into());
        self
    }

    pub fn with_filter_text(mut self, filter_text: impl Into<SharedString>) -> Self {
        self.filter_text = Some(filter_text.into());
        self
    }

    pub fn with_preselect(mut self, preselect: bool) -> Self {
        self.preselect = preselect;
        self
    }

    pub fn with_source_index(mut self, source_index: usize) -> Self {
        self.source_index = source_index;
        self
    }

    pub fn with_selection_priority(mut self, selection_priority: u64) -> Self {
        self.selection_priority = selection_priority;
        self
    }

    pub fn with_server_id(mut self, server_id: Option<u64>) -> Self {
        self.server_id = server_id;
        self
    }

    pub fn with_raw_lsp_item(mut self, raw_lsp_item: Option<serde_json::Value>) -> Self {
        self.raw_lsp_item = raw_lsp_item;
        self
    }

    pub fn with_locality_score(mut self, locality_score: u16) -> Self {
        self.locality_score = locality_score;
        self
    }
}

/// Position tracking for query optimization
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

/// Enhanced completion view with async filtering and smart query optimization
pub struct CompletionView {
    // Data Management
    all_items: Vec<CompletionItem>,
    match_candidates: Vec<StringMatchCandidate>,
    filtered_entries: Vec<StringMatch>,

    // State Tracking
    initial_query: Option<String>,
    initial_position: Option<Position>,
    current_query: Option<String>,

    // Async Processing
    filter_task: Option<Task<Vec<StringMatch>>>,
    cancel_flag: Arc<AtomicBool>,

    // UI State
    selected_index: usize,
    visible: bool,

    // Configuration
    show_documentation: bool,
    sort_completions: bool,
    max_items: usize,

    // Performance Optimization
    cache: CompletionCache,
    debouncer: CompletionDebouncer,
    items_hash: u64,
    performance_monitor: PerformanceMonitor,
    // Error Handling
    error_handler: CompletionErrorHandler,
    last_error: Option<ErrorContext>,

    // Advanced UI Components
    documentation_loader: DocumentationLoader,
    documentation_panel: DocumentationPanel,
    list_state: CompletionListState,
    current_documentation: Option<DocumentationContent>,

    // GPUI Requirements
    focus_handle: FocusHandle,
}

impl CompletionView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        println!("COMP: Creating new CompletionView");
        let focus_handle = cx.focus_handle();
        // Register completion focus with the global coordinator if available
        if let Some(coord) = cx.try_global::<crate::FocusCoordinator>() {
            coord.set_completion_focus(focus_handle.clone());
        }

        Self {
            all_items: Vec::new(),
            match_candidates: Vec::new(),
            filtered_entries: Vec::new(),
            initial_query: None,
            initial_position: None,
            current_query: None,
            filter_task: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            selected_index: 0,
            visible: false,
            show_documentation: true,
            sort_completions: true,
            max_items: 50,
            cache: CompletionCache::new(),
            debouncer: create_completion_debouncer(),
            items_hash: 0,
            performance_monitor: PerformanceMonitor::new(),
            error_handler: CompletionErrorHandler::new(),
            last_error: None,
            documentation_loader: DocumentationLoader::new(DocumentationCacheConfig::default()),
            documentation_panel: DocumentationPanel::new(),
            list_state: CompletionListState::new(
                COMPLETION_ROW_HEIGHT_PX,
                COMPLETION_LIST_MAX_HEIGHT_PX,
            ),
            current_documentation: None,
            focus_handle,
        }
    }

    /// Set all completion items and prepare candidates for filtering
    pub fn set_items(&mut self, items: Vec<CompletionItem>, cx: &mut Context<Self>) {
        self.set_items_with_filter(items, None, cx);
    }

    /// Set all completion items with an optional initial filter
    pub fn set_items_with_filter(
        &mut self,
        items: Vec<CompletionItem>,
        initial_filter: Option<String>,
        cx: &mut Context<Self>,
    ) {
        // Log the incoming completion data
        nucleotide_logging::debug!(
            item_count = items.len(),
            has_filter = initial_filter.is_some(),
            filter = %initial_filter.as_deref().unwrap_or(""),
            "Setting completion items with filter"
        );

        // Enhanced completion data ready for display

        // Log first few completion items for debugging
        if !items.is_empty() {
            let sample_items: Vec<String> = items
                .iter()
                .take(5)
                .map(|item| item.text.to_string())
                .collect();
            nucleotide_logging::debug!(
                sample_count = sample_items.len(),
                sample_items = ?sample_items,
                total_items = items.len(),
                "Sample of completion items before filtering"
            );
        }

        // Calculate hash for cache invalidation
        let new_hash = self.calculate_items_hash(&items);

        // If items haven't changed, no need to update
        if new_hash == self.items_hash && !self.all_items.is_empty() {
            nucleotide_logging::debug!("Items unchanged, skipping update");
            return;
        }

        // Invalidate cache for old items
        if self.items_hash != 0 {
            self.cache.invalidate_items(self.items_hash);
        }

        self.all_items = items;
        self.items_hash = new_hash;

        // Prepare match candidates
        self.match_candidates = self
            .all_items
            .iter()
            .enumerate()
            .map(|(index, item)| StringMatchCandidate::from_item(index, item))
            .collect();

        // Reset state
        nucleotide_logging::debug!("Resetting completion state and making visible");
        self.filtered_entries.clear();
        self.selected_index = 0;
        self.initial_query = None;
        self.initial_position = None;
        self.current_query = None;

        // Cancel any ongoing filtering and reset debouncer
        self.cancel_current_filter();
        self.debouncer.reset();

        // Apply initial filter if provided, otherwise show all items
        if !self.all_items.is_empty() {
            if let Some(filter) = initial_filter {
                nucleotide_logging::debug!(
                    filter = %filter,
                    "Applying initial filter to completion items"
                );
                // Apply the initial filter immediately
                self.filter_immediate(filter, None, cx);
            } else {
                nucleotide_logging::debug!("No filter provided, showing all items");
                let results = self
                    .match_candidates
                    .iter()
                    .map(|candidate| StringMatch::for_candidate(candidate, 100, Vec::new()))
                    .collect();
                self.apply_filtered_entries(results);
                self.visible = true;

                nucleotide_logging::debug!(
                    total_items = self.all_items.len(),
                    filtered_items = self.filtered_entries.len(),
                    "Completion items set without filtering"
                );
            }
        } else {
            nucleotide_logging::warn!("No completion items provided, hiding completion menu");
            self.visible = false;
        }

        // Update performance monitor
        self.performance_monitor
            .update_memory_usage(self.all_items.len(), self.cache.size());
        cx.notify();
    }

    /// Calculate a hash for the completion items to detect changes
    fn calculate_items_hash(&self, items: &[CompletionItem]) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        for item in items {
            item.text.hash(&mut hasher);
            if let Some(ref desc) = item.description {
                desc.hash(&mut hasher);
            }
            if let Some(ref display) = item.display_text {
                display.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Check if a new query is just an extension of the previous one
    fn is_query_extension(&self, new_query: &str) -> bool {
        match &self.initial_query {
            Some(initial) => new_query.starts_with(initial),
            None => false,
        }
    }

    /// Determine if we should refilter based on query and position changes
    fn should_refilter(&self, new_query: &str, new_position: Option<&Position>) -> bool {
        match (&self.initial_query, &self.initial_position) {
            (Some(initial_query), Some(initial_pos)) => {
                // Always refilter if position changed
                if let Some(new_pos) = new_position
                    && new_pos != initial_pos
                {
                    return true;
                }

                // If query is not an extension, refilter
                !new_query.starts_with(initial_query)
            }
            _ => true, // Always refilter if no baseline
        }
    }

    /// Cancel the current filtering task
    pub fn cancel_current_filter(&mut self) {
        self.cancel_flag.store(true, Ordering::Relaxed);
        self.filter_task = None;
        // Create new cancel flag for next operation
        self.cancel_flag = Arc::new(AtomicBool::new(false));
    }

    /// Start async filtering with the given query (with debouncing)
    pub fn filter_async(
        &mut self,
        query: String,
        position: Option<Position>,
        cx: &mut Context<Self>,
    ) {
        // For now, implement debouncing with immediate filtering
        // This provides a complete implementation without complex async handling
        // Future versions can add proper async debouncing with proper GPUI patterns

        // Simple debouncing: only filter if query has changed significantly
        if let Some(ref current_query) = self.current_query
            && current_query == &query
        {
            return; // No change, skip filtering
        }

        // Apply filtering immediately with debouncing logic
        self.filter_immediate(query, position, cx);
    }

    /// Update the completion filter when the user's typing prefix changes
    /// This is the main method that should be called when the user types/deletes characters
    pub fn update_filter(&mut self, new_prefix: String, cx: &mut Context<Self>) {
        nucleotide_logging::debug!(
            prefix = %new_prefix,
            current_query = ?self.current_query,
            "Updating completion filter with new prefix"
        );

        // Use the async filter method which handles debouncing and optimization
        self.filter_async(new_prefix, None, cx);
    }

    /// Update the completion filter with both prefix and cursor position
    /// Useful when both the typed text and cursor position have changed
    pub fn update_filter_with_position(
        &mut self,
        new_prefix: String,
        position: Position,
        cx: &mut Context<Self>,
    ) {
        nucleotide_logging::debug!(
            prefix = %new_prefix,
            position = ?position,
            "Updating completion filter with new prefix and position"
        );

        // Use the async filter method which handles debouncing and optimization
        self.filter_async(new_prefix, Some(position), cx);
    }

    /// Immediate filtering without debouncing (for internal use)
    fn filter_immediate(
        &mut self,
        query: String,
        position: Option<Position>,
        cx: &mut Context<Self>,
    ) {
        nucleotide_logging::debug!(
            query = %query,
            position = ?position,
            total_items = self.all_items.len(),
            current_filtered = self.filtered_entries.len(),
            "Starting immediate filtering"
        );

        let timer = PerformanceTimer::start("filter_immediate");

        // Check for very large item counts that might cause performance issues
        if self.all_items.len() > 10000 {
            nucleotide_logging::warn!(
                item_count = self.all_items.len(),
                "Too many completion items, triggering resource error"
            );
            self.handle_error(
                CompletionError::ResourceError {
                    resource_type: "completion_items".to_string(),
                    current_usage: self.all_items.len() as u64,
                    limit: 10000,
                },
                cx,
            );
            return;
        }

        // Check cache first
        let cache_key = CacheKey::new(query.clone(), position.clone(), self.items_hash);
        if let Some(cached_results) = self.cache.get(&cache_key) {
            let (_, duration) = timer.stop();
            self.performance_monitor
                .record_filter(duration, true, false);

            nucleotide_logging::debug!(
                query = %query,
                cached_results = cached_results.len(),
                duration_ms = duration.as_millis(),
                "Using cached filter results"
            );

            self.apply_filtered_entries(cached_results);
            self.visible = !self.filtered_entries.is_empty();
            self.current_query = Some(query);
            self.update_list_state();
            self.update_documentation_for_selection(cx);
            cx.notify();
            return;
        }

        // Check if we can optimize using query extension
        if !self.should_refilter(&query, position.as_ref()) {
            nucleotide_logging::debug!(
                query = %query,
                "Optimizing by filtering existing results"
            );
            // Can optimize by filtering existing results
            self.filter_existing_results(&query, cx);
            return;
        }

        // Cancel any ongoing filter
        self.cancel_current_filter();

        // Store initial state if this is the first filter
        if self.initial_query.is_none() {
            self.initial_query = Some(query.clone());
            self.initial_position = position.clone();
        }

        self.current_query = Some(query.clone());

        // If query is empty, show all items
        if query.is_empty() {
            nucleotide_logging::debug!(
                total_items = self.match_candidates.len(),
                max_items = self.max_items,
                "Empty query, showing all items"
            );

            let results: Vec<StringMatch> = self
                .match_candidates
                .iter()
                .map(|candidate| StringMatch::for_candidate(candidate, 100, Vec::new()))
                .collect();

            // Cache the results
            self.cache.insert(cache_key, results.clone());

            nucleotide_logging::debug!(
                filtered_count = results.len(),
                "Filtered results for empty query"
            );

            self.apply_filtered_entries(results);
            self.visible = !self.filtered_entries.is_empty();
            self.update_list_state();
            self.update_documentation_for_selection(cx);
            cx.notify();
            return;
        }

        nucleotide_logging::debug!(
            query = %query,
            candidates = self.match_candidates.len(),
            "Performing full filtering with query"
        );

        // Check if we can use optimization base from cache
        if let Some(base_results) = self.try_optimization_from_cache(&query) {
            nucleotide_logging::debug!(
                query = %query,
                base_results = base_results.len(),
                "Using optimization base from cache"
            );
            // Filter the base results for the new query
            let optimized_results = self.filter_cached_results(&base_results, &query);
            self.cache.insert(cache_key, optimized_results.clone());
            self.apply_filtered_entries(optimized_results);
            self.visible = !self.filtered_entries.is_empty();
            self.update_list_state();
            self.update_documentation_for_selection(cx);
            cx.notify();
            return;
        }

        // Perform synchronous filtering for immediate results
        let candidates = &self.match_candidates;
        let max_items = self.max_items;

        nucleotide_logging::debug!(
            query = %query,
            candidates = candidates.len(),
            "Performing synchronous filtering with fuzzy matching"
        );

        let query_lower = query.to_lowercase();
        let mut matched_count = 0;
        let mut filtered_matches: Vec<StringMatch> = Vec::with_capacity(max_items);

        nucleotide_logging::debug!(
            query = %query,
            query_lower = %query_lower,
            candidate_count = candidates.len(),
            "Starting filtering loop"
        );

        for (idx, candidate) in candidates.iter().enumerate() {
            if idx < 5 {
                nucleotide_logging::debug!(
                    idx = idx,
                    candidate_text = %candidate.text,
                    candidate_id = candidate.id,
                    query_lower = %query_lower,
                    "Checking candidate"
                );
            }

            if let Some(string_match) = fuzzy_match_candidate(candidate, &query_lower) {
                matched_count += 1;
                if idx < 5 {
                    nucleotide_logging::debug!(
                        candidate_text = %candidate.text,
                        score = string_match.score,
                        "MATCHED: fuzzy match"
                    );
                }
                filtered_matches.push(string_match);
            } else {
                if idx < 5 {
                    nucleotide_logging::debug!(
                        candidate_text = %candidate.text,
                        "NO MATCH"
                    );
                }
            }
        }

        nucleotide_logging::debug!(
            total_candidates = candidates.len(),
            matched_count = matched_count,
            filtered_matches = filtered_matches.len(),
            max_items = max_items,
            query = %query,
            "Filtering completed with detailed results"
        );

        // Cache the results
        self.cache.insert(cache_key, filtered_matches.clone());

        // Apply the filtered results immediately
        self.apply_filtered_entries(filtered_matches);

        // Set visible to true if we have matches
        self.visible = !self.filtered_entries.is_empty();

        // Check and log visibility state
        let is_visible = self.is_visible();
        nucleotide_logging::debug!(
            filtered_entries = self.filtered_entries.len(),
            visible_flag = self.visible,
            is_visible_result = is_visible,
            query = %query,
            "Completion view visibility status after filtering"
        );

        self.update_list_state();
        self.update_documentation_for_selection(cx);

        let (_, duration) = timer.stop();
        self.performance_monitor
            .record_filter(duration, false, false);

        cx.notify();
    }

    /// Try to get optimization base from cache
    fn try_optimization_from_cache(&mut self, query: &str) -> Option<Arc<[StringMatch]>> {
        // Look for shorter queries that we can build upon
        for len in query_prefix_boundaries_desc(query) {
            let base_query = &query[..len];
            if let Some(results) = self
                .cache
                .get_optimization_base(base_query, self.items_hash)
            {
                return Some(results);
            }
        }
        None
    }

    fn candidate_for_match(&self, string_match: &StringMatch) -> Option<&StringMatchCandidate> {
        self.match_candidates
            .get(string_match.item_index)
            .filter(|candidate| candidate.id == string_match.candidate_id)
            .or_else(|| {
                self.match_candidates
                    .iter()
                    .find(|candidate| candidate.id == string_match.candidate_id)
            })
    }

    fn item_for_match(&self, string_match: &StringMatch) -> Option<&CompletionItem> {
        self.all_items.get(string_match.item_index).or_else(|| {
            self.candidate_for_match(string_match)
                .and_then(|candidate| self.all_items.get(candidate.item_index))
        })
    }

    fn apply_filtered_entries(&mut self, mut matches: Vec<StringMatch>) {
        self.sort_matches(&mut matches);
        matches.truncate(self.max_items);
        self.selected_index = self.initial_selection_index(&matches);
        self.filtered_entries = matches;
    }

    fn sort_matches(&self, matches: &mut [StringMatch]) {
        if !self.sort_completions {
            return;
        }

        matches.sort_by(|left, right| self.compare_match_rank(left, right));
    }

    fn compare_match_rank(&self, left: &StringMatch, right: &StringMatch) -> CmpOrdering {
        right
            .score
            .cmp(&left.score)
            .then_with(
                || match (self.item_for_match(left), self.item_for_match(right)) {
                    (Some(left_item), Some(right_item)) => {
                        right_item.locality_score.cmp(&left_item.locality_score)
                    }
                    (Some(_), None) => CmpOrdering::Less,
                    (None, Some(_)) => CmpOrdering::Greater,
                    (None, None) => CmpOrdering::Equal,
                },
            )
            .then_with(
                || match (self.item_for_match(left), self.item_for_match(right)) {
                    (Some(left_item), Some(right_item)) => {
                        compare_completion_items(left_item, right_item)
                    }
                    (Some(_), None) => CmpOrdering::Less,
                    (None, Some(_)) => CmpOrdering::Greater,
                    (None, None) => left.item_index.cmp(&right.item_index),
                },
            )
            .then_with(|| left.item_index.cmp(&right.item_index))
    }

    fn initial_selection_index(&self, matches: &[StringMatch]) -> usize {
        let Some(first_match) = matches.first() else {
            return 0;
        };

        let top_score = first_match.score;
        matches
            .iter()
            .take_while(|string_match| string_match.score == top_score)
            .position(|string_match| {
                self.item_for_match(string_match)
                    .is_some_and(|item| item.preselect)
            })
            .or_else(|| {
                matches
                    .iter()
                    .enumerate()
                    .filter_map(|(index, string_match)| {
                        self.item_for_match(string_match)
                            .map(|item| (index, item.selection_priority))
                    })
                    .filter(|(_, priority)| *priority > 0)
                    .max_by_key(|(_, priority)| *priority)
                    .map(|(index, _)| index)
            })
            .unwrap_or(0)
    }

    /// Filter cached results for a more specific query
    fn filter_cached_results(
        &self,
        cached_results: &[StringMatch],
        query: &str,
    ) -> Vec<StringMatch> {
        let query_lower = query.to_lowercase();

        cached_results
            .iter()
            .filter_map(|string_match| {
                self.candidate_for_match(string_match)
                    .and_then(|candidate| fuzzy_match_candidate(candidate, &query_lower))
            })
            .collect()
    }

    /// Filter existing results for query extensions (optimization)
    fn filter_existing_results(&mut self, query: &str, cx: &mut Context<Self>) {
        // For query extensions, filter the existing results
        if self.is_query_extension(query) {
            let query_lower = query.to_lowercase();

            let filtered_entries = std::mem::take(&mut self.filtered_entries)
                .into_iter()
                .filter_map(|string_match| {
                    self.match_candidates
                        .get(string_match.item_index)
                        .filter(|candidate| candidate.id == string_match.candidate_id)
                        .or_else(|| {
                            self.match_candidates
                                .iter()
                                .find(|candidate| candidate.id == string_match.candidate_id)
                        })
                        .and_then(|candidate| fuzzy_match_candidate(candidate, &query_lower))
                })
                .collect();

            self.apply_filtered_entries(filtered_entries);
            self.current_query = Some(query.to_string());
            self.update_list_state();
            self.update_documentation_for_selection(cx);
            cx.notify();
        } else {
            // Fall back to full refiltering
            self.filter_async(query.to_string(), self.initial_position.clone(), cx);
        }
    }

    /// Update the view with completed filter results
    pub fn update_filtered_items(&mut self, matches: Vec<StringMatch>, cx: &mut Context<Self>) {
        let total_before = self.all_items.len();
        let filtered_count = matches.len();

        nucleotide_logging::debug!(
            total_items = total_before,
            filtered_items = filtered_count,
            filter_ratio = if total_before > 0 { (filtered_count as f32 / total_before as f32) * 100.0 } else { 0.0 },
            current_query = %self.current_query.as_deref().unwrap_or(""),
            "Filter results updated"
        );

        // Log sample of filtered results
        if !matches.is_empty() {
            let sample_items: Vec<String> = matches
                .iter()
                .take(5)
                .filter_map(|string_match| {
                    self.item_for_match(string_match)
                        .map(|item| format!("{}({})", item.text, string_match.score))
                })
                .collect();

            nucleotide_logging::debug!(
                sample_results = ?sample_items,
                sample_count = sample_items.len(),
                "Sample of filtered completion items with scores"
            );
        }

        self.apply_filtered_entries(matches);
        self.visible = !self.filtered_entries.is_empty();
        self.filter_task = None;
        self.update_list_state();
        self.scroll_to_selection();
        self.update_documentation_for_selection(cx);
        cx.notify();
    }

    /// Get the currently selected completion item
    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.filtered_entries
            .get(self.selected_index)
            .and_then(|string_match| self.item_for_match(string_match))
    }

    /// Get completion item at specific index
    pub fn get_item_at_index(&self, index: usize) -> Option<&CompletionItem> {
        self.filtered_entries
            .get(index)
            .and_then(|string_match| self.item_for_match(string_match))
    }

    /// Get the currently selected item index in the filtered list
    pub fn selected_index(&self) -> Option<usize> {
        if self.selected_index < self.filtered_entries.len() {
            Some(self.selected_index)
        } else {
            None
        }
    }

    /// Move selection up/down
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!(
            current_index = self.selected_index,
            filtered_count = self.filtered_entries.len(),
            "select_next called"
        );
        if !self.filtered_entries.is_empty() {
            let old_index = self.selected_index;
            self.selected_index = (self.selected_index + 1) % self.filtered_entries.len();
            nucleotide_logging::debug!(
                old_index = old_index,
                new_index = self.selected_index,
                "select_next: changed selection"
            );
            self.scroll_to_selection();
            self.update_documentation_for_selection(cx);
            cx.notify();
        } else {
            nucleotide_logging::warn!("select_next: no filtered entries available");
        }
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!(
            current_index = self.selected_index,
            filtered_count = self.filtered_entries.len(),
            "select_prev called"
        );
        if !self.filtered_entries.is_empty() {
            let old_index = self.selected_index;
            self.selected_index = if self.selected_index == 0 {
                self.filtered_entries.len() - 1
            } else {
                self.selected_index - 1
            };
            nucleotide_logging::debug!(
                old_index = old_index,
                new_index = self.selected_index,
                "select_prev: changed selection"
            );
            self.scroll_to_selection();
            self.update_documentation_for_selection(cx);
            cx.notify();
        } else {
            nucleotide_logging::warn!("select_prev: no filtered entries available");
        }
    }

    /// Hide the completion view
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.cancel_current_filter();
        self.all_items.clear();
        self.match_candidates.clear();
        self.filtered_entries.clear();
        self.initial_query = None;
        self.initial_position = None;
        self.current_query = None;
        self.selected_index = 0;
        cx.notify();
    }

    /// Check if the completion view is visible
    pub fn is_visible(&self) -> bool {
        self.visible && (!self.filtered_entries.is_empty() || self.filter_task.is_some())
    }

    /// Get the number of filtered items
    pub fn item_count(&self) -> usize {
        self.filtered_entries.len()
    }

    /// Update documentation for the currently selected item
    fn update_documentation_for_selection(&mut self, cx: &mut Context<Self>) {
        let selected_item = self.selected_item().cloned();

        if let Some(item) = selected_item {
            self.current_documentation = self.documentation_loader.load_documentation(&item, cx);
            self.documentation_panel
                .set_content(self.current_documentation.clone());
        } else {
            self.current_documentation = None;
            self.documentation_panel.set_content(None);
        }
    }

    /// Update the list state with current item count
    fn update_list_state(&mut self) {
        self.list_state
            .update_item_count(self.filtered_entries.len());
    }

    /// Scroll to make the currently selected item visible
    fn scroll_to_selection(&mut self) {
        if self.selected_index < self.filtered_entries.len() {
            nucleotide_logging::debug!(
                selected_index = self.selected_index,
                total_items = self.filtered_entries.len(),
                "Scrolling to selected completion item"
            );

            // Calculate scroll position for manual scrolling
            let item_height = self.list_state.item_height;
            let container_height = self.list_state.max_height;
            let selected_position = (self.selected_index as f32) * item_height;

            // Calculate if item is outside visible area
            let visible_items = (container_height / item_height).floor() as usize;
            let scroll_position = if self.selected_index < visible_items / 2 {
                // Near top - scroll to top
                0.0
            } else if self.selected_index
                >= self
                    .filtered_entries
                    .len()
                    .saturating_sub(visible_items / 2)
            {
                // Near bottom - scroll to bottom
                (self.filtered_entries.len().saturating_sub(visible_items) as f32 * item_height)
                    .max(0.0)
            } else {
                // Center the selected item
                selected_position - (container_height / 2.0)
            };

            nucleotide_logging::debug!(
                selected_position = selected_position,
                scroll_position = scroll_position,
                item_height = item_height,
                container_height = container_height,
                "Calculated scroll position for selection"
            );

            // Note: For now, we rely on GPUI's built-in scroll behavior
            // A future enhancement could implement programmatic scrolling
            self.list_state.scroll_to_item(self.selected_index);
        }
    }

    /// Move selection by a page (multiple items at once)
    pub fn select_page_down(&mut self, cx: &mut Context<Self>) {
        if self.filtered_entries.is_empty() {
            return;
        }

        let page_size = (self.list_state.max_height / self.list_state.item_height).floor() as usize;
        let old_index = self.selected_index;
        self.selected_index =
            (self.selected_index + page_size).min(self.filtered_entries.len() - 1);

        if old_index != self.selected_index {
            nucleotide_logging::debug!(
                old_index = old_index,
                new_index = self.selected_index,
                page_size = page_size,
                "select_page_down: changed selection"
            );
            self.scroll_to_selection();
            self.update_documentation_for_selection(cx);
            cx.notify();
        }
    }

    /// Move selection up by a page (multiple items at once)
    pub fn select_page_up(&mut self, cx: &mut Context<Self>) {
        if self.filtered_entries.is_empty() {
            return;
        }

        let page_size = (self.list_state.max_height / self.list_state.item_height).floor() as usize;
        let old_index = self.selected_index;
        self.selected_index = self.selected_index.saturating_sub(page_size);

        if old_index != self.selected_index {
            nucleotide_logging::debug!(
                old_index = old_index,
                new_index = self.selected_index,
                page_size = page_size,
                "select_page_up: changed selection"
            );
            self.scroll_to_selection();
            self.update_documentation_for_selection(cx);
            cx.notify();
        }
    }

    /// Jump to first item
    pub fn select_first(&mut self, cx: &mut Context<Self>) {
        if !self.filtered_entries.is_empty() && self.selected_index != 0 {
            let old_index = self.selected_index;
            self.selected_index = 0;
            nucleotide_logging::debug!(
                old_index = old_index,
                new_index = self.selected_index,
                "select_first: changed selection"
            );
            self.scroll_to_selection();
            self.update_documentation_for_selection(cx);
            cx.notify();
        }
    }

    /// Jump to last item
    pub fn select_last(&mut self, cx: &mut Context<Self>) {
        if !self.filtered_entries.is_empty() {
            let old_index = self.selected_index;
            let last_index = self.filtered_entries.len() - 1;
            if self.selected_index != last_index {
                self.selected_index = last_index;
                nucleotide_logging::debug!(
                    old_index = old_index,
                    new_index = self.selected_index,
                    "select_last: changed selection"
                );
                self.scroll_to_selection();
                self.update_documentation_for_selection(cx);
                cx.notify();
            }
        }
    }

    fn confirm_selected_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(selected_index) = self.selected_index() else {
            nucleotide_logging::warn!("Completion confirm requested but no item is selected");
            return false;
        };

        nucleotide_logging::debug!(
            selected_index = selected_index,
            "Accepting selected completion via Helix"
        );
        cx.emit(CompleteViaHelixEvent {
            item_index: selected_index,
        });
        cx.emit(DismissEvent);
        true
    }

    fn confirm_completion_action(
        &mut self,
        _: &CompletionConfirm,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_selected_completion(cx);
    }

    fn confirm_completion_and_stop_action(
        &mut self,
        _: &CompletionConfirmAndStop,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.confirm_selected_completion(cx) {
            cx.stop_propagation();
        }
    }

    fn dismiss_completion_action(
        &mut self,
        _: &CompletionDismiss,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(DismissEvent);
        cx.stop_propagation();
    }

    fn select_previous_action(
        &mut self,
        _: &CompletionSelectPrev,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_prev(cx);
        cx.stop_propagation();
    }

    fn select_next_action(
        &mut self,
        _: &CompletionSelectNext,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_next(cx);
        cx.stop_propagation();
    }

    fn select_page_up_action(
        &mut self,
        _: &CompletionPageUp,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_page_up(cx);
        cx.stop_propagation();
    }

    fn select_page_down_action(
        &mut self,
        _: &CompletionPageDown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_page_down(cx);
        cx.stop_propagation();
    }

    fn select_first_action(
        &mut self,
        _: &CompletionSelectFirst,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_first(cx);
        cx.stop_propagation();
    }

    fn select_last_action(
        &mut self,
        _: &CompletionSelectLast,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_last(cx);
        cx.stop_propagation();
    }

    /// Get the list state for rendering
    pub fn list_state(&self) -> &CompletionListState {
        &self.list_state
    }

    /// Set documentation panel visibility
    pub fn set_documentation_visible(&mut self, visible: bool) {
        self.documentation_panel.set_visible(visible);
    }

    // Error Handling Methods

    /// Handle an error with automatic recovery
    pub fn handle_error(&mut self, error: CompletionError, cx: &mut Context<Self>) {
        let result = self.error_handler.handle_error(error);

        match result {
            ErrorHandlingResult::Continue => {
                // Log and continue normal operation
            }
            ErrorHandlingResult::ShowWarning(context) => {
                self.last_error = Some(context.clone());
                if let Some(message) = &context.user_message {
                    cx.emit(CompletionWarningEvent {
                        message: SharedString::from(message.clone()),
                    });
                    nucleotide_logging::warn!(message = %message, "Completion warning");
                }
            }
            ErrorHandlingResult::Recover(action, context) => {
                self.last_error = Some(context.clone());
                self.execute_recovery_action(action, cx);
            }
            ErrorHandlingResult::Degrade(context) => {
                self.last_error = Some(context.clone());
                self.enter_degraded_mode(cx);
            }
            ErrorHandlingResult::Shutdown(context) => {
                self.last_error = Some(context);
                self.shutdown_completion_system(cx);
            }
        }
    }

    /// Execute a recovery action
    fn execute_recovery_action(
        &mut self,
        action: crate::completion_error::RecoveryAction,
        cx: &mut Context<Self>,
    ) {
        use crate::completion_error::RecoveryAction;

        match action {
            RecoveryAction::Retry {
                delay,
                max_attempts: _,
            } => {
                let delay_duration = delay;
                cx.spawn(async move |this, cx| {
                    tokio::time::sleep(delay_duration).await;
                    let _ = this.update(cx, |view, cx| view.retry_last_filter(cx));
                })
                .detach();
            }
            RecoveryAction::Fallback {
                action: _,
                description,
            } => {
                // Switch to basic completion mode
                nucleotide_logging::debug!(description = %description, "Fallback activated");
                self.enter_fallback_mode();
            }
            RecoveryAction::ClearCache { cache_types: _ } => {
                // Clear the completion cache
                self.cache.clear();
            }
            RecoveryAction::OfflineMode { duration: _ } => {
                // Disable network-dependent features
                self.enter_offline_mode();
            }
            RecoveryAction::RestartComponent { component: _ } => {
                // Restart the completion system
                self.restart_completion_system(cx);
            }
            RecoveryAction::NotifyUser {
                message,
                action_text: _,
            } => {
                // Show user notification
                nucleotide_logging::debug!(message = %message, "Completion system notification");
            }
        }
    }

    fn retry_last_filter(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(query) = self.current_query.clone() else {
            nucleotide_logging::debug!("Skipping completion retry without a current query");
            return false;
        };

        let position = self.initial_position.clone();
        nucleotide_logging::debug!(
            query = %query,
            position = ?position,
            "Retrying completion filter"
        );
        self.filter_immediate(query, position, cx);
        true
    }

    /// Enter degraded mode with reduced functionality
    fn enter_degraded_mode(&mut self, cx: &mut Context<Self>) {
        self.max_items = 10; // Reduce item count
        self.show_documentation = false; // Disable documentation
        self.cache.clear(); // Clear cache to free memory
        cx.notify();
    }

    /// Enter fallback mode with basic completion only
    fn enter_fallback_mode(&mut self) {
        self.sort_completions = false;
        self.show_documentation = false;
        self.max_items = 5;
    }

    /// Enter offline mode
    fn enter_offline_mode(&mut self) {
        // Disable network-dependent features
        self.show_documentation = false;
    }

    /// Restart the completion system
    fn restart_completion_system(&mut self, cx: &mut Context<Self>) {
        // Cancel any running tasks
        self.cancel_current_filter();

        // Reset state
        self.filtered_entries.clear();
        self.selected_index = 0;
        self.cache.clear();
        self.last_error = None;

        // Reset configuration to defaults
        self.show_documentation = true;
        self.sort_completions = true;
        self.max_items = 50;

        cx.notify();
    }

    /// Shutdown the completion system
    fn shutdown_completion_system(&mut self, cx: &mut Context<Self>) {
        self.cancel_current_filter();
        self.visible = false;
        self.filtered_entries.clear();
        cx.notify();
    }

    /// Get the last error that occurred
    pub fn last_error(&self) -> Option<&ErrorContext> {
        self.last_error.as_ref()
    }

    /// Clear the last error
    pub fn clear_last_error(&mut self) {
        self.last_error = None;
    }

    /// Check if error rate is concerning
    pub fn is_error_rate_high(&self) -> bool {
        self.error_handler.is_error_rate_high()
    }

    /// Get error statistics
    pub fn error_stats(&self) -> crate::completion_error::ErrorStats {
        self.error_handler
            .get_error_stats(std::time::Duration::from_secs(300))
    }

    // Performance Monitoring Methods

    /// Get current performance metrics
    pub fn performance_metrics(&self) -> &PerformanceMonitor {
        &self.performance_monitor
    }

    /// Check if performance is degraded
    pub fn is_performance_degraded(&self) -> bool {
        let recommendations = self.performance_monitor.get_recommendations();
        !recommendations.is_empty()
    }

    /// Get performance recommendations
    pub fn get_performance_recommendations(&self) -> Vec<String> {
        self.performance_monitor.get_recommendations()
    }

    /// Optimize performance based on current metrics
    pub fn optimize_performance(&mut self) {
        let recommendations = self.performance_monitor.get_recommendations();

        for recommendation in recommendations {
            match recommendation.as_str() {
                "Reduce max items" => {
                    self.max_items = (self.max_items * 3 / 4).max(10);
                }
                "Disable documentation" => {
                    self.show_documentation = false;
                }
                "Clear cache" => {
                    self.cache.clear();
                }
                "Disable sorting" => {
                    self.sort_completions = false;
                }
                _ => {}
            }
        }
    }

    /// Monitor memory usage and take action if needed
    pub fn monitor_memory_usage(&mut self, cx: &mut Context<Self>) {
        let current_memory = self.estimate_memory_usage();

        // If memory usage is too high, take corrective action
        if current_memory > 50 * 1024 * 1024 {
            // 50MB threshold
            self.handle_error(
                CompletionError::ResourceError {
                    resource_type: "memory".to_string(),
                    current_usage: current_memory,
                    limit: 50 * 1024 * 1024,
                },
                cx,
            );
        }
    }

    /// Estimate current memory usage
    fn estimate_memory_usage(&self) -> u64 {
        // Rough estimation of memory usage
        let items_memory = self.all_items.len() * 200; // ~200 bytes per item
        let candidates_memory = self.match_candidates.len() * 100; // ~100 bytes per candidate
        let filtered_memory = self.filtered_entries.len() * 50; // ~50 bytes per match
        let cache_memory = self.cache.size() * 300; // ~300 bytes per cache entry

        (items_memory + candidates_memory + filtered_memory + cache_memory) as u64
    }

    fn selected_documentation_preview(&self) -> Option<String> {
        let item = self.selected_item()?;

        for text in [&item.documentation, &item.detail, &item.description]
            .into_iter()
            .flatten()
        {
            let preview = normalize_completion_preview(text);
            if !preview.is_empty() {
                return Some(preview);
            }
        }

        None
    }

    /// Tune performance parameters based on system capabilities
    pub fn tune_performance_parameters(&mut self) {
        // Adjust parameters based on current performance
        let avg_filter_time = self.performance_monitor.get_average_filter_time();

        if avg_filter_time > std::time::Duration::from_millis(100) {
            // Filtering is too slow, reduce scope
            self.max_items = (self.max_items * 3 / 4).max(5);

            if avg_filter_time > std::time::Duration::from_millis(200) {
                // Very slow, disable expensive features
                self.show_documentation = false;
                self.sort_completions = false;
            }
        } else if avg_filter_time < std::time::Duration::from_millis(20) {
            // Filtering is fast, can increase scope
            self.max_items = (self.max_items * 5 / 4).min(100);
        }
    }
}

fn normalize_completion_preview(text: &SharedString) -> String {
    let mut preview = String::new();
    let mut in_code_block = false;

    for raw_line in text.lines() {
        let mut line = raw_line.trim();

        if line.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        if line.is_empty() {
            continue;
        }

        if !in_code_block {
            line = line.trim_start_matches('#').trim();
            line = line.trim_start_matches("- ").trim();
        }

        if !preview.is_empty() {
            preview.push(' ');
        }
        preview.push_str(line);
    }

    let preview = preview.replace(['`', '*'], "");
    let mut compact = preview.split_whitespace().collect::<Vec<_>>().join(" ");

    const MAX_PREVIEW_CHARS: usize = 420;
    if compact.chars().count() > MAX_PREVIEW_CHARS {
        compact = compact.chars().take(MAX_PREVIEW_CHARS).collect();
        compact.push_str("...");
    }

    compact
}

impl Focusable for CompletionView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for CompletionView {}
impl EventEmitter<CompleteViaHelixEvent> for CompletionView {}
impl EventEmitter<CompletionWarningEvent> for CompletionView {}

impl Render for CompletionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        nucleotide_logging::debug!(
            visible = self.is_visible(),
            all_items_count = self.all_items.len(),
            filtered_count = self.filtered_entries.len(),
            "🎨 COMPLETION_VIEW RENDER: Start"
        );

        nucleotide_logging::debug!(
            visible = self.is_visible(),
            all_items_count = self.all_items.len(),
            filtered_count = self.filtered_entries.len(),
            selected_index = self.selected_index,
            "CompletionView render start"
        );

        if !self.is_visible() {
            nucleotide_logging::debug!(
                "🎨 COMPLETION_VIEW RENDER: Not visible, returning empty div"
            );
            return div().id("completion-hidden");
        }

        // Access theme - if not available, return empty
        let theme = match cx.try_global::<crate::Theme>() {
            Some(theme) => theme,
            None => {
                println!("🎨 COMPLETION_VIEW RENDER: No theme found, returning empty div");
                return div().id("completion-no-theme");
            }
        };
        let tokens = &theme.tokens;

        // Store item count for uniform_list - processor will access view data directly
        let filtered_entries = &self.filtered_entries;
        let selected_documentation = self.selected_documentation_preview();

        // Use flexible layout - let container size itself based on content.
        let max_visible_items = COMPLETION_VISIBLE_ROWS;

        // Calculate maximum container height to prevent it from growing too large
        let max_container_height = px(COMPLETION_LIST_MAX_HEIGHT_PX);

        nucleotide_logging::debug!(
            filtered_count = filtered_entries.len(),
            max_visible_items = max_visible_items,
            max_container_height = f32::from(max_container_height),
            "Using flexible layout for completion list"
        );

        // Focusable container with key handling for completion navigation
        let container = div()
            .id("completion-popup-v2")
            .focusable()
            .key_context(COMPLETION_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::dismiss_completion_action))
            .on_action(cx.listener(Self::confirm_completion_action))
            .on_action(cx.listener(Self::confirm_completion_and_stop_action))
            .on_action(cx.listener(Self::select_previous_action))
            .on_action(cx.listener(Self::select_next_action))
            .on_action(cx.listener(Self::select_page_up_action))
            .on_action(cx.listener(Self::select_page_down_action))
            .on_action(cx.listener(Self::select_first_action))
            .on_action(cx.listener(Self::select_last_action));

        // Do NOT steal focus from the editor - completion should be a non-modal overlay
        // The editor needs to maintain focus for proper keyboard event handling

        // Positioning is controlled by the parent overlay, which anchors this
        // non-modal view to the editor cursor.
        nucleotide_logging::debug!(
            item_count = self.filtered_entries.len(),
            "Completion view render completed"
        );
        container
            .absolute()
            // Remove hardcoded positioning - parent will handle this
            .child(
                div()
                    .id("completion-popup-layout")
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(tokens.sizes.space_2)
                    .child(
                        div()
                            .id("completion-list")
                            .flex()
                            .flex_col()
                            // Calculate optimal width based on content
                            .w({
                                // Calculate optimal width based on the longest item
                                let base_width = 220.0; // Minimum practical width

                                // Find the longest text in visible items
                                let max_text_length = self
                                    .filtered_entries
                                    .iter()
                                    .take(max_visible_items)
                                    .map(|string_match| {
                                        self.item_for_match(string_match)
                                            .map(|item| {
                                                // Calculate total text length including signature and type info
                                                let mut total_len = item
                                                    .display_text
                                                    .as_ref()
                                                    .map_or(item.text.len(), |text| text.len());

                                                if let Some(sig) = &item.signature_info {
                                                    total_len += sig.len();
                                                }
                                                if let Some(type_info) = &item.type_info {
                                                    total_len += type_info.len() + 1;
                                                }
                                                if let Some(detail) = &item.detail {
                                                    total_len += detail.len() + 1;
                                                }
                                                total_len
                                            })
                                            .unwrap_or(0)
                                    })
                                    .max()
                                    .unwrap_or(0);

                                // Estimate pixel width (rough approximation: 8px per character)
                                let estimated_width: f32 =
                                    base_width + (max_text_length as f32 * 7.5);

                                // Cap the width to reasonable bounds
                                let optimal_width = estimated_width.min(840.0).max(base_width);

                                nucleotide_logging::debug!(
                                    max_text_length = max_text_length,
                                    estimated_width = estimated_width,
                                    optimal_width = optimal_width,
                                    "Calculated dynamic completion width"
                                );

                                px(optimal_width)
                            })
                            .bg(tokens.chrome.popup_background)
                            .border_1()
                            .border_color(tokens.chrome.popup_border)
                            .rounded(tokens.sizes.radius_md)
                            .shadow(vec![
                                tokens.chrome.shadow_md.to_box_shadow(false),
                                tokens.chrome.inset_highlight.to_box_shadow(true),
                            ])
                            .py(tokens.sizes.space_1)
                            .px(tokens.sizes.space_1)
                            .child(
                                div()
                                    .id("completion-list-container")
                                    .flex()
                                    .flex_col()
                                    .w_full()
                                    .max_h(max_container_height) // Flexible height with maximum constraint
                                    .min_h(px(28.0)) // Allow single-item height without excessive padding
                                    .bg(tokens.chrome.popup_background) // Ensure background is visible
                                    .child(
                                        // Scrollable container with working completion items
                                        div()
                                            .id("completion-scrollable-container")
                                            .flex()
                                            .flex_col()
                                            .w_full()
                                            // Let height be content-driven; don't force full container height
                                            .max_h(max_container_height)
                                            .overflow_y_scroll() // Enable scrolling
                                            .children({
                                                // Implement a sliding window of visible items centered around selection
                                                let total_items = self.filtered_entries.len();
                                                let max_visible = max_visible_items;

                                                let (start_index, end_index) =
                                                    if total_items <= max_visible {
                                                        // Show all items if we have few enough
                                                        nucleotide_logging::debug!(
                                                            "Using full list - items fit in window"
                                                        );
                                                        (0, total_items)
                                                    } else {
                                                        // Need a sliding window
                                                        let selected = self.selected_index;

                                                        nucleotide_logging::debug!(
                                                            selected = selected,
                                                            total_items = total_items,
                                                            max_visible = max_visible,
                                                            "Calculating sliding window"
                                                        );

                                                        // Simple logic: ensure selected item is always visible
                                                        let start = if selected
                                                            < max_visible.saturating_sub(1)
                                                        {
                                                            // Selected is near beginning, show from start
                                                            0
                                                        } else if selected
                                                            >= total_items.saturating_sub(1)
                                                        {
                                                            // Selected is last item, show last window
                                                            total_items.saturating_sub(max_visible)
                                                        } else {
                                                            // Selected is in middle, center it
                                                            let half = max_visible / 2;
                                                            selected.saturating_sub(half)
                                                        };

                                                        // Ensure we don't go past the end
                                                        let start = start.min(
                                                            total_items.saturating_sub(max_visible),
                                                        );
                                                        let end =
                                                            (start + max_visible).min(total_items);

                                                        nucleotide_logging::debug!(
                                                            calculated_start = start,
                                                            calculated_end = end,
                                                            "Calculated window bounds"
                                                        );

                                                        (start, end)
                                                    };

                                                // Verify the selected item is actually in the window
                                                let selected_visible = self.selected_index
                                                    >= start_index
                                                    && self.selected_index < end_index;

                                                nucleotide_logging::debug!(
                                                    total_items = total_items,
                                                    selected_index = self.selected_index,
                                                    start_index = start_index,
                                                    end_index = end_index,
                                                    window_size = end_index - start_index,
                                                    selected_visible = selected_visible,
                                                    "Rendering completion window"
                                                );

                                                if !selected_visible {
                                                    nucleotide_logging::error!(
                                                        selected_index = self.selected_index,
                                                        start_index = start_index,
                                                        end_index = end_index,
                                                        total_items = total_items,
                                                        max_visible = max_visible,
                                                        "SELECTED ITEM NOT IN VISIBLE WINDOW!"
                                                    );
                                                }

                                                // Special debugging for last few items
                                                if self.selected_index
                                                    >= total_items.saturating_sub(3)
                                                {
                                                    nucleotide_logging::debug!(
                                                        selected_index = self.selected_index,
                                                        total_items = total_items,
                                                        start_index = start_index,
                                                        end_index = end_index,
                                                        is_last_item = self.selected_index
                                                            == total_items.saturating_sub(1),
                                                        is_second_last = self.selected_index
                                                            == total_items.saturating_sub(2),
                                                        "Debugging last few items"
                                                    );
                                                }

                                                let items_to_render_count =
                                                    end_index.saturating_sub(start_index);

                                                nucleotide_logging::debug!(
                                                    items_to_render_count = items_to_render_count,
                                                    expected_count = end_index - start_index,
                                                    first_index = (items_to_render_count > 0)
                                                        .then_some(start_index),
                                                    last_index = (items_to_render_count > 0)
                                                        .then_some(end_index - 1),
                                                    "Items actually being rendered"
                                                );

                                                (start_index..end_index)
                                                    .filter_map(|index| {
                                                        let string_match =
                                                            self.filtered_entries.get(index)?;
                                                        let item =
                                                            self.item_for_match(string_match)?;
                                                        let is_selected =
                                                            index == self.selected_index;

                                                        // Add explicit ID for scroll-to-element functionality
                                                        let completion_element =
                                                            CompletionItemElement::new(
                                                                item.clone(),
                                                                string_match.clone(),
                                                                is_selected,
                                                            );

                                                        // Wrap in div with ID for scroll targeting
                                                        Some(
                                                            div()
                                                                .id((
                                                                    "completion-item-wrapper",
                                                                    index,
                                                                ))
                                                                .w_full()
                                                                .child(
                                                                    completion_element
                                                                        .compact()
                                                                        .into_element_with_theme(
                                                                            theme,
                                                                        ),
                                                                ),
                                                        )
                                                    })
                                                    .collect::<Vec<_>>()
                                            }),
                                    ),
                            ),
                    )
                    .when_some(selected_documentation, |layout, documentation| {
                        layout.child(
                            div()
                                .id("completion-documentation-preview")
                                .flex()
                                .flex_col()
                                .w(px(390.0))
                                .max_w(px(460.0))
                                .max_h(max_container_height)
                                .overflow_hidden()
                                .p(tokens.sizes.space_3)
                                .bg(tokens.chrome.popup_background)
                                .border_1()
                                .border_color(tokens.chrome.popup_border)
                                .rounded(tokens.sizes.radius_md)
                                .shadow(vec![
                                    tokens.chrome.shadow_md.to_box_shadow(false),
                                    tokens.chrome.inset_highlight.to_box_shadow(true),
                                ])
                                .text_size(tokens.sizes.text_base)
                                .line_height(relative(1.35))
                                .text_color(tokens.chrome.text_on_chrome)
                                .child(documentation),
                        )
                    }),
            )
    }
}

// Unit tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_match_candidate_creation() {
        let item = CompletionItem::new("test_function")
            .with_description("A test function")
            .with_display_text("test_function()");

        let candidate = StringMatchCandidate::from(&item);
        assert_eq!(candidate.text, "test_function()");
        assert!(candidate.id > 0); // Should have generated an ID
    }

    #[test]
    fn test_string_match_ordering() {
        let match1 = StringMatch::new(1, 100, vec![0, 1, 2]);
        let match2 = StringMatch::new(2, 200, vec![0, 1]);
        let match3 = StringMatch::new(3, 150, vec![0, 2]);

        let mut matches = [match1, match2, match3];
        matches.sort();

        // Should be sorted by score descending
        assert_eq!(matches[0].score, 200);
        assert_eq!(matches[1].score, 150);
        assert_eq!(matches[2].score, 100);
    }

    #[test]
    fn test_fuzzy_match_scores_prefix_above_substring() {
        let prefix = StringMatchCandidate::with_index(1, "format".to_string(), 0);
        let substring = StringMatchCandidate::with_index(2, "debug_format".to_string(), 1);

        let prefix_match = fuzzy_match_candidate(&prefix, "fo").expect("prefix match");
        let substring_match = fuzzy_match_candidate(&substring, "fo").expect("substring match");

        assert!(prefix_match.score > substring_match.score);
        assert_eq!(prefix_match.positions, vec![0, 1]);
    }

    #[test]
    fn test_fuzzy_match_supports_subsequence_matches() {
        let candidate = StringMatchCandidate::with_index(1, "HashMap".to_string(), 0);

        let string_match = fuzzy_match_candidate(&candidate, "hmp").expect("subsequence match");

        assert_eq!(string_match.positions, vec![0, 4, 6]);
        assert!(string_match.score > 0);
    }

    #[test]
    fn query_prefix_boundaries_desc_uses_utf8_boundaries() {
        let prefixes: Vec<&str> = query_prefix_boundaries_desc("éclair")
            .map(|boundary| &"éclair"[..boundary])
            .collect();

        assert_eq!(prefixes, vec!["éclai", "écla", "écl", "éc", "é"]);
    }

    #[test]
    fn completion_menu_keys_match_helix_completion_navigation() {
        assert_eq!(
            completion_menu_action_for_key("tab", false, false),
            Some(CompletionMenuAction::Confirm)
        );
        assert_eq!(
            completion_menu_action_for_key("y", true, false),
            Some(CompletionMenuAction::Confirm)
        );
        assert_eq!(
            completion_menu_action_for_key("down", false, false),
            Some(CompletionMenuAction::SelectNext)
        );
        assert_eq!(
            completion_menu_action_for_key("n", true, false),
            Some(CompletionMenuAction::SelectNext)
        );
        assert_eq!(
            completion_menu_action_for_key("up", false, false),
            Some(CompletionMenuAction::SelectPrevious)
        );
        assert_eq!(
            completion_menu_action_for_key("p", true, false),
            Some(CompletionMenuAction::SelectPrevious)
        );
        assert_eq!(
            completion_menu_action_for_key("escape", false, false),
            Some(CompletionMenuAction::Dismiss)
        );
    }

    #[test]
    fn completion_menu_keys_ignore_non_helix_completion_bindings() {
        assert_eq!(completion_menu_action_for_key("enter", false, false), None);
        assert_eq!(completion_menu_action_for_key("tab", false, true), None);
        assert_eq!(completion_menu_action_for_key("c", true, false), None);
        assert_eq!(completion_menu_action_for_key("down", true, false), None);
    }

    #[test]
    fn test_query_extension_detection() {
        // Create a minimal test view without GPUI context
        let view = TestCompletionView {
            initial_query: Some("test".to_string()),
        };

        assert!(view.is_query_extension("test_func"));
        assert!(view.is_query_extension("testing"));
        assert!(!view.is_query_extension("func"));
        assert!(!view.is_query_extension("other"));
    }

    // Test helper struct that doesn't require GPUI context
    struct TestCompletionView {
        initial_query: Option<String>,
    }

    impl TestCompletionView {
        fn is_query_extension(&self, new_query: &str) -> bool {
            match &self.initial_query {
                Some(initial) => new_query.starts_with(initial),
                None => false,
            }
        }
    }

    #[test]
    fn test_position_equality() {
        let pos1 = Position::new(10, 5);
        let pos2 = Position::new(10, 5);
        let pos3 = Position::new(10, 6);

        assert_eq!(pos1, pos2);
        assert_ne!(pos1, pos3);
    }

    #[test]
    fn test_completion_item_builder() {
        let item = CompletionItem::new("function_name")
            .with_description("A cool function")
            .with_kind(CompletionItemKind::Function)
            .with_documentation("Detailed documentation here")
            .with_filter_text("function")
            .with_sort_text("001")
            .with_preselect(true)
            .with_source_index(7)
            .with_selection_priority(42)
            .with_server_id(Some(9))
            .with_raw_lsp_item(Some(serde_json::json!({
                "label": "function_name",
                "data": { "provider": "rust-analyzer" }
            })))
            .with_locality_score(25);

        assert_eq!(item.text, "function_name");
        assert_eq!(item.description.as_ref().unwrap(), "A cool function");
        assert_eq!(item.kind, Some(CompletionItemKind::Function));
        assert_eq!(
            item.documentation.as_ref().unwrap(),
            "Detailed documentation here"
        );
        assert_eq!(item.filter_text.as_ref().unwrap(), "function");
        assert_eq!(item.sort_text.as_ref().unwrap(), "001");
        assert!(item.preselect);
        assert_eq!(item.source_index, 7);
        assert_eq!(item.selection_priority, 42);
        assert_eq!(item.server_id, Some(9));
        assert_eq!(
            item.raw_lsp_item,
            Some(serde_json::json!({
                "label": "function_name",
                "data": { "provider": "rust-analyzer" }
            }))
        );
        assert_eq!(item.locality_score, 25);
    }

    #[test]
    fn test_should_refilter_logic() {
        let view = TestCompletionViewExtended {
            initial_query: Some("test".to_string()),
            initial_position: Some(Position::new(10, 5)),
        };

        // Same position, query extension - should not refilter
        assert!(!view.should_refilter("testing", Some(&Position::new(10, 5))));

        // Same position, different query - should refilter
        assert!(view.should_refilter("other", Some(&Position::new(10, 5))));

        // Different position - should refilter
        assert!(view.should_refilter("testing", Some(&Position::new(10, 6))));

        // No initial state - should refilter
        let empty_view = TestCompletionViewExtended {
            initial_query: None,
            initial_position: None,
        };
        assert!(empty_view.should_refilter("test", Some(&Position::new(10, 5))));
    }

    // Extended test helper for more complex logic
    struct TestCompletionViewExtended {
        initial_query: Option<String>,
        initial_position: Option<Position>,
    }

    impl TestCompletionViewExtended {
        fn should_refilter(&self, new_query: &str, new_position: Option<&Position>) -> bool {
            match (&self.initial_query, &self.initial_position) {
                (Some(initial_query), Some(initial_pos)) => {
                    // Always refilter if position changed
                    if let Some(new_pos) = new_position
                        && new_pos != initial_pos
                    {
                        return true;
                    }

                    // If query is not an extension, refilter
                    !new_query.starts_with(initial_query)
                }
                _ => true, // Always refilter if no baseline
            }
        }
    }

    #[test]
    fn test_candidate_id_generation() {
        // Test that different texts generate different IDs
        let item1 = CompletionItem::new("function_a");
        let item2 = CompletionItem::new("function_b");

        let candidate1 = StringMatchCandidate::from(&item1);
        let candidate2 = StringMatchCandidate::from(&item2);

        assert_ne!(candidate1.id, candidate2.id);

        // Test that same text generates same ID (deterministic)
        let item3 = CompletionItem::new("function_a");
        let candidate3 = StringMatchCandidate::from(&item3);

        assert_eq!(candidate1.id, candidate3.id);
    }

    #[test]
    fn test_completion_item_kinds() {
        // Test that all completion item kinds work correctly
        let kinds = vec![
            CompletionItemKind::Text,
            CompletionItemKind::Method,
            CompletionItemKind::Function,
            CompletionItemKind::Constructor,
            CompletionItemKind::Field,
            CompletionItemKind::Variable,
            CompletionItemKind::Class,
            CompletionItemKind::Interface,
            CompletionItemKind::Module,
            CompletionItemKind::Property,
            CompletionItemKind::Unit,
            CompletionItemKind::Value,
            CompletionItemKind::Enum,
            CompletionItemKind::Keyword,
            CompletionItemKind::Snippet,
            CompletionItemKind::Color,
            CompletionItemKind::File,
            CompletionItemKind::Reference,
            CompletionItemKind::Folder,
            CompletionItemKind::EnumMember,
            CompletionItemKind::Constant,
            CompletionItemKind::Struct,
            CompletionItemKind::Event,
            CompletionItemKind::Operator,
            CompletionItemKind::TypeParameter,
        ];

        for kind in kinds {
            let item = CompletionItem::new("test").with_kind(kind);
            assert_eq!(item.kind, Some(kind));
        }
    }

    mod filter_tests {
        use super::*;
        use gpui::TestAppContext;

        #[gpui::test]
        async fn completion_list_state_uses_six_visible_rows(cx: &mut TestAppContext) {
            let (completion_view, _cx) = cx.add_window_view(|_window, cx| CompletionView::new(cx));

            completion_view.update(cx, |view, _cx| {
                assert_eq!(view.list_state.item_height, COMPLETION_ROW_HEIGHT_PX);
                assert_eq!(view.list_state.max_height, COMPLETION_LIST_MAX_HEIGHT_PX);
                assert_eq!(
                    (view.list_state.max_height / view.list_state.item_height).floor() as usize,
                    COMPLETION_VISIBLE_ROWS
                );
            });
        }

        #[gpui::test]
        async fn test_set_items_with_filter_basic(cx: &mut TestAppContext) {
            // Test that set_items_with_filter correctly applies an initial filter

            let completion_items = vec![
                CompletionItem::new("println!").with_kind(CompletionItemKind::Function),
                CompletionItem::new("print!").with_kind(CompletionItemKind::Function),
                CompletionItem::new("format!").with_kind(CompletionItemKind::Function),
                CompletionItem::new("vec!").with_kind(CompletionItemKind::Function),
            ];

            let (completion_view, _cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                // Test filtering with "pri" prefix - should match print! and println!
                view.set_items_with_filter(completion_items.clone(), Some("pri".to_string()), cx);
                view
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, _cx| {
                // Should only show items matching "pri"
                let filtered_count = view.filtered_entries.len();
                assert!(
                    filtered_count <= 2,
                    "Expected 2 or fewer items matching 'pri', got {}",
                    filtered_count
                );
                assert!(
                    filtered_count > 0,
                    "Expected at least 1 item matching 'pri', got {}",
                    filtered_count
                );
            });
        }

        #[gpui::test]
        async fn test_set_items_with_filter_empty_prefix(cx: &mut TestAppContext) {
            // Test that empty prefix shows all items

            let completion_items = vec![
                CompletionItem::new("alpha").with_kind(CompletionItemKind::Function),
                CompletionItem::new("beta").with_kind(CompletionItemKind::Function),
                CompletionItem::new("gamma").with_kind(CompletionItemKind::Function),
            ];

            let (completion_view, _cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                // Test with empty filter - should show all items
                view.set_items_with_filter(completion_items.clone(), Some("".to_string()), cx);
                view
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, _cx| {
                let filtered_count = view.filtered_entries.len();
                assert_eq!(
                    filtered_count, 3,
                    "Expected all 3 items with empty filter, got {}",
                    filtered_count
                );
            });
        }

        #[gpui::test]
        async fn test_set_items_with_filter_no_matches(cx: &mut TestAppContext) {
            // Test that non-matching prefix results in no items

            let completion_items = vec![
                CompletionItem::new("alpha").with_kind(CompletionItemKind::Function),
                CompletionItem::new("beta").with_kind(CompletionItemKind::Function),
                CompletionItem::new("gamma").with_kind(CompletionItemKind::Function),
            ];

            let (completion_view, _cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                // Test with non-matching filter
                view.set_items_with_filter(completion_items.clone(), Some("xyz".to_string()), cx);
                view
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, _cx| {
                let filtered_count = view.filtered_entries.len();
                assert_eq!(
                    filtered_count, 0,
                    "Expected no items matching 'xyz', got {}",
                    filtered_count
                );
            });
        }

        #[gpui::test]
        async fn test_set_items_with_filter_none(cx: &mut TestAppContext) {
            // Test that None filter shows all items (no initial filtering)

            let completion_items = vec![
                CompletionItem::new("test1").with_kind(CompletionItemKind::Function),
                CompletionItem::new("test2").with_kind(CompletionItemKind::Function),
                CompletionItem::new("other").with_kind(CompletionItemKind::Function),
            ];

            let (completion_view, _cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                // Test with None filter - should show all items
                view.set_items_with_filter(completion_items.clone(), None, cx);
                view
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, _cx| {
                let filtered_count = view.filtered_entries.len();
                assert_eq!(
                    filtered_count, 3,
                    "Expected all 3 items with None filter, got {}",
                    filtered_count
                );
            });
        }

        #[gpui::test]
        async fn test_update_filter_refines_existing_completion_results(cx: &mut TestAppContext) {
            let completion_items = vec![
                CompletionItem::new("print()").with_kind(CompletionItemKind::Function),
                CompletionItem::new("print").with_kind(CompletionItemKind::Function),
                CompletionItem::new("println").with_kind(CompletionItemKind::Function),
                CompletionItem::new("process").with_kind(CompletionItemKind::Function),
                CompletionItem::new("format").with_kind(CompletionItemKind::Function),
            ];

            let (completion_view, _cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                view.set_items_with_filter(completion_items.clone(), None, cx);
                view
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, cx| {
                assert_eq!(view.item_count(), 5);
                view.update_filter("pr".to_string(), cx);
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, cx| {
                let mut labels: Vec<_> = (0..view.item_count())
                    .filter_map(|index| view.get_item_at_index(index))
                    .map(|item| item.text.to_string())
                    .collect();
                labels.sort();

                assert_eq!(labels, vec!["print", "print()", "println", "process"]);
                view.update_filter("print".to_string(), cx);
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, cx| {
                let mut labels: Vec<_> = (0..view.item_count())
                    .filter_map(|index| view.get_item_at_index(index))
                    .map(|item| item.text.to_string())
                    .collect();
                labels.sort();

                assert_eq!(labels, vec!["print", "print()", "println"]);
                view.update_filter("printl".to_string(), cx);
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, _cx| {
                let mut labels: Vec<_> = (0..view.item_count())
                    .filter_map(|index| view.get_item_at_index(index))
                    .map(|item| item.text.to_string())
                    .collect();
                labels.sort();

                assert_eq!(labels, vec!["println"]);
            });
        }

        #[gpui::test]
        async fn test_retry_last_filter_reapplies_current_query(cx: &mut TestAppContext) {
            let completion_items = vec![
                CompletionItem::new("print").with_kind(CompletionItemKind::Function),
                CompletionItem::new("println").with_kind(CompletionItemKind::Function),
                CompletionItem::new("format").with_kind(CompletionItemKind::Function),
            ];

            let (completion_view, _cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                view.set_items_with_filter(completion_items.clone(), None, cx);
                view
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, cx| {
                view.update_filter("print".to_string(), cx);
                view.filtered_entries.clear();

                assert!(view.retry_last_filter(cx));
                let mut labels: Vec<_> = (0..view.item_count())
                    .filter_map(|index| view.get_item_at_index(index))
                    .map(|item| item.text.to_string())
                    .collect();
                labels.sort();

                assert_eq!(labels, vec!["print", "println"]);
            });
        }

        #[gpui::test]
        async fn test_set_items_with_filter_fuzzy_matching(cx: &mut TestAppContext) {
            // Test fuzzy matching behavior

            let completion_items = vec![
                CompletionItem::new("print_debug").with_kind(CompletionItemKind::Function),
                CompletionItem::new("println_macro").with_kind(CompletionItemKind::Function),
                CompletionItem::new("format_string").with_kind(CompletionItemKind::Function),
                CompletionItem::new("prefix_test").with_kind(CompletionItemKind::Function),
            ];

            let (completion_view, _cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                // Test fuzzy matching with "pr" - should match print_debug, println_macro, prefix_test
                view.set_items_with_filter(completion_items.clone(), Some("pr".to_string()), cx);
                view
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, _cx| {
                let filtered_count = view.filtered_entries.len();
                // Should match multiple items that contain "pr" characters
                assert!(
                    filtered_count >= 2,
                    "Expected at least 2 items matching fuzzy 'pr', got {}",
                    filtered_count
                );
                // Should not match all items
                assert!(
                    filtered_count < 4,
                    "Expected fewer than 4 items matching 'pr', got {}",
                    filtered_count
                );
            });
        }

        #[gpui::test]
        async fn test_filter_uses_lsp_ranking_metadata(cx: &mut TestAppContext) {
            let completion_items = vec![
                CompletionItem::new("alpha")
                    .with_kind(CompletionItemKind::Function)
                    .with_sort_text("999"),
                CompletionItem::new("fmt(...)")
                    .with_kind(CompletionItemKind::Method)
                    .with_filter_text("debug_fmt")
                    .with_sort_text("001")
                    .with_preselect(true),
                CompletionItem::new("debug_print")
                    .with_kind(CompletionItemKind::Function)
                    .with_sort_text("010"),
            ];

            let (completion_view, _cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                view.set_items_with_filter(completion_items.clone(), Some("debug".to_string()), cx);
                view
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, _cx| {
                let labels: Vec<_> = (0..view.item_count())
                    .filter_map(|index| view.get_item_at_index(index))
                    .map(|item| item.label_text().to_string())
                    .collect();

                assert_eq!(labels, vec!["fmt(...)", "debug_print"]);
                assert_eq!(view.selected_index, 0);
            });
        }

        #[gpui::test]
        async fn test_selection_priority_selects_recent_match(cx: &mut TestAppContext) {
            let completion_items = vec![
                CompletionItem::new("foo").with_kind(CompletionItemKind::Function),
                CompletionItem::new("foobar")
                    .with_kind(CompletionItemKind::Function)
                    .with_selection_priority(5),
            ];

            let (completion_view, _cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                view.set_items_with_filter(completion_items.clone(), Some("foo".to_string()), cx);
                view
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, _cx| {
                assert_eq!(view.selected_index, 1);
                assert_eq!(
                    view.selected_item().map(|item| item.text.to_string()),
                    Some("foobar".to_string())
                );
            });
        }

        #[gpui::test]
        async fn test_completion_view_visibility(cx: &mut TestAppContext) {
            // Test that CompletionView is visible when it has items and invisible when empty

            let completion_items =
                vec![CompletionItem::new("test").with_kind(CompletionItemKind::Function)];

            let (completion_view, _cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                // Initially should not be visible
                assert!(!view.visible, "CompletionView should start invisible");

                // After setting items, should be visible
                view.set_items_with_filter(completion_items.clone(), None, cx);
                view
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, _cx| {
                assert!(
                    view.visible,
                    "CompletionView should be visible after setting items"
                );
                assert!(
                    !view.filtered_entries.is_empty(),
                    "Should have filtered entries"
                );
            });
        }

        #[gpui::test]
        async fn test_completion_view_selection(cx: &mut TestAppContext) {
            // Test selection behavior

            let completion_items = vec![
                CompletionItem::new("first").with_kind(CompletionItemKind::Function),
                CompletionItem::new("second").with_kind(CompletionItemKind::Function),
                CompletionItem::new("third").with_kind(CompletionItemKind::Function),
            ];

            let (completion_view, _cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                view.set_items_with_filter(completion_items.clone(), None, cx);
                view
            });

            cx.run_until_parked();

            completion_view.update(cx, |view, _cx| {
                // Should start with first item selected
                assert_eq!(
                    view.selected_index, 0,
                    "Should start with first item selected"
                );

                // Test that we can move selection
                view.select_next(_cx);
                assert_eq!(view.selected_index, 1, "Should move to second item");

                view.select_prev(_cx);
                assert_eq!(view.selected_index, 0, "Should move back to first item");
            });
        }

        #[gpui::test]
        async fn test_completion_view_handles_navigation_actions(cx: &mut TestAppContext) {
            cx.update(|cx| {
                cx.set_global(crate::Theme::from_tokens(crate::DesignTokens::dark()));
            });

            let completion_items = vec![
                CompletionItem::new("first").with_kind(CompletionItemKind::Function),
                CompletionItem::new("second").with_kind(CompletionItemKind::Function),
                CompletionItem::new("third").with_kind(CompletionItemKind::Function),
            ];

            let (completion_view, cx) = cx.add_window_view(|_window, cx| {
                let mut view = CompletionView::new(cx);
                view.set_items_with_filter(completion_items.clone(), None, cx);
                view
            });
            let focus = completion_view.read_with(cx, |view, _| view.focus_handle.clone());

            cx.update(|window, cx| {
                window.focus(&focus, cx);
                focus.dispatch_action(&CompletionSelectNext, window, cx);
            });

            completion_view.read_with(cx, |view, _| {
                assert_eq!(view.selected_index, 1);
            });

            cx.update(|window, cx| {
                focus.dispatch_action(&CompletionSelectPrev, window, cx);
            });

            completion_view.read_with(cx, |view, _| {
                assert_eq!(view.selected_index, 0);
            });
        }
    }
}
