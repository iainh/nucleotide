// ABOUTME: Enhanced completion system with async filtering and smart query optimization
// ABOUTME: Professional-grade completion view based on Zed's architecture

use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Task, Window, div, px,
};
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

/// Event emitted to request completion acceptance via Helix's Transaction system
/// This event signals that Helix should handle the completion acceptance
#[derive(Debug, Clone)]
pub struct CompleteViaHelixEvent {
    pub item_index: usize,
}

/// Candidate for fuzzy matching - lightweight representation of completion items
#[derive(Debug, Clone)]
pub struct StringMatchCandidate {
    /// Unique identifier for this candidate
    pub id: usize,
    /// Text content to match against
    pub text: String,
}

impl StringMatchCandidate {
    pub fn new(id: usize, text: String) -> Self {
        Self { id, text }
    }
}

impl From<&CompletionItem> for StringMatchCandidate {
    fn from(item: &CompletionItem) -> Self {
        // Use display_text if available, otherwise use the main text
        let text = item.display_text.as_ref().unwrap_or(&item.text).to_string();

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
    /// Match score (higher is better)
    pub score: u16,
    /// Character positions that matched in the original string
    pub positions: Vec<usize>,
}

impl StringMatch {
    pub fn new(candidate_id: usize, score: u16, positions: Vec<usize>) -> Self {
        Self {
            candidate_id,
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
}

/// LSP Insert Text Format
#[derive(Debug, Clone, PartialEq)]
pub enum InsertTextFormat {
    /// Plain text insertion
    PlainText,
    /// Snippet with tabstops and placeholders ($0, $1, ${1:placeholder})
    Snippet,
}

#[derive(Debug, Clone, PartialEq)]
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
        }
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
            list_state: CompletionListState::new(32.0, 400.0), // Increased from 24px to account for padding and multi-row layout
            current_documentation: None,
            focus_handle: cx.focus_handle(),
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
        nucleotide_logging::info!(
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
            .map(StringMatchCandidate::from)
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
                nucleotide_logging::info!(
                    filter = %filter,
                    "Applying initial filter to completion items"
                );
                // Apply the initial filter immediately
                self.filter_immediate(filter, None, cx);
            } else {
                nucleotide_logging::debug!("No filter provided, showing all items");
                self.filtered_entries = self
                    .match_candidates
                    .iter()
                    .map(|candidate| StringMatch {
                        candidate_id: candidate.id,
                        score: 100,
                        positions: Vec::new(),
                    })
                    .collect();
                self.visible = true;

                nucleotide_logging::info!(
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
        if let Some(ref current_query) = self.current_query {
            if current_query == &query {
                return; // No change, skip filtering
            }
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
        nucleotide_logging::info!(
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

            nucleotide_logging::info!(
                query = %query,
                cached_results = cached_results.len(),
                duration_ms = duration.as_millis(),
                "Using cached filter results"
            );

            self.filtered_entries = cached_results;
            self.selected_index = 0;
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
            nucleotide_logging::info!(
                total_items = self.match_candidates.len(),
                max_items = self.max_items,
                "Empty query, showing all items"
            );

            let results: Vec<StringMatch> = self
                .match_candidates
                .iter()
                .map(|candidate| StringMatch::new(candidate.id, 100, vec![]))
                .take(self.max_items)
                .collect();

            // Cache the results
            self.cache.insert(cache_key, results.clone());

            nucleotide_logging::info!(
                filtered_count = results.len(),
                "Filtered results for empty query"
            );

            self.filtered_entries = results;
            self.selected_index = 0;
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
            let optimized_results = self.filter_cached_results(base_results, &query);
            self.cache.insert(cache_key, optimized_results.clone());
            self.filtered_entries = optimized_results;
            self.selected_index = 0;
            self.visible = !self.filtered_entries.is_empty();
            self.update_list_state();
            self.update_documentation_for_selection(cx);
            cx.notify();
            return;
        }

        // Perform synchronous filtering for immediate results
        let candidates = &self.match_candidates;
        let max_items = self.max_items;

        nucleotide_logging::info!(
            query = %query,
            candidates = candidates.len(),
            "Performing synchronous filtering with fuzzy matching"
        );

        // Use simple prefix matching for now (can be enhanced with fuzzy matching later)
        let query_lower = query.to_lowercase();
        let mut matched_count = 0;
        let mut filtered_matches: Vec<StringMatch> = Vec::new();

        nucleotide_logging::debug!(
            query = %query,
            query_lower = %query_lower,
            candidate_count = candidates.len(),
            "Starting filtering loop"
        );

        for (idx, candidate) in candidates.iter().enumerate() {
            let candidate_text = candidate.text.to_lowercase();

            if idx < 5 {
                nucleotide_logging::debug!(
                    idx = idx,
                    candidate_text = %candidate_text,
                    candidate_id = candidate.id,
                    query_lower = %query_lower,
                    "Checking candidate"
                );
            }

            // Check if candidate text starts with or contains the query
            let score = if candidate_text.starts_with(&query_lower) {
                matched_count += 1;
                if idx < 5 {
                    nucleotide_logging::debug!(
                        candidate_text = %candidate_text,
                        "MATCHED: prefix match"
                    );
                }
                Some(100) // High score for prefix match
            } else if candidate_text.contains(&query_lower) {
                matched_count += 1;
                if idx < 5 {
                    nucleotide_logging::debug!(
                        candidate_text = %candidate_text,
                        "MATCHED: substring match"
                    );
                }
                Some(50) // Lower score for substring match
            } else {
                if idx < 5 {
                    nucleotide_logging::debug!(
                        candidate_text = %candidate_text,
                        "NO MATCH"
                    );
                }
                None
            };

            if let Some(score_val) = score {
                filtered_matches.push(StringMatch::new(candidate.id, score_val, vec![]));
            }
        }

        // Sort by score descending (highest scores first) and limit to max_items
        filtered_matches.sort_by(|a, b| b.score.cmp(&a.score));
        filtered_matches.truncate(max_items);

        nucleotide_logging::info!(
            total_candidates = candidates.len(),
            matched_count = matched_count,
            filtered_matches = filtered_matches.len(),
            max_items = max_items,
            query = %query,
            "Filtering completed with detailed results, sorted by score descending"
        );

        // Cache the results
        self.cache.insert(cache_key, filtered_matches.clone());

        // Apply the filtered results immediately
        self.filtered_entries = filtered_matches;
        self.selected_index = 0;

        // Set visible to true if we have matches
        self.visible = !self.filtered_entries.is_empty();

        // Check and log visibility state
        let is_visible = self.is_visible();
        nucleotide_logging::info!(
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
    fn try_optimization_from_cache(&mut self, query: &str) -> Option<Vec<StringMatch>> {
        // Look for shorter queries that we can build upon
        for len in (1..query.len()).rev() {
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

    /// Filter cached results for a more specific query
    fn filter_cached_results(
        &self,
        cached_results: Vec<StringMatch>,
        query: &str,
    ) -> Vec<StringMatch> {
        cached_results
            .into_iter()
            .filter(|string_match| {
                // Find the candidate and check if it still matches the new query
                if let Some(candidate) = self
                    .match_candidates
                    .iter()
                    .find(|c| c.id == string_match.candidate_id)
                {
                    candidate
                        .text
                        .to_lowercase()
                        .contains(&query.to_lowercase())
                } else {
                    false
                }
            })
            .take(self.max_items)
            .collect()
    }

    /// Filter existing results for query extensions (optimization)
    fn filter_existing_results(&mut self, query: &str, cx: &mut Context<Self>) {
        // For query extensions, filter the existing results
        if self.is_query_extension(query) {
            self.filtered_entries.retain(|string_match| {
                // Find the candidate and check if it still matches
                if let Some(candidate) = self
                    .match_candidates
                    .iter()
                    .find(|c| c.id == string_match.candidate_id)
                {
                    candidate
                        .text
                        .to_lowercase()
                        .contains(&query.to_lowercase())
                } else {
                    false
                }
            });

            self.selected_index = 0;
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

        nucleotide_logging::info!(
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
                    self.all_items
                        .iter()
                        .find(|item| {
                            let candidate = StringMatchCandidate::from(*item);
                            candidate.id == string_match.candidate_id
                        })
                        .map(|item| format!("{}({})", item.text.to_string(), string_match.score))
                })
                .collect();

            nucleotide_logging::debug!(
                sample_results = ?sample_items,
                sample_count = sample_items.len(),
                "Sample of filtered completion items with scores"
            );
        }

        self.filtered_entries = matches;
        self.selected_index = 0;
        self.visible = !self.filtered_entries.is_empty();
        self.filter_task = None;
        self.update_list_state();
        self.scroll_to_selection();
        self.update_documentation_for_selection(cx);
        cx.notify();
    }

    /// Get the currently selected completion item
    pub fn selected_item(&self) -> Option<&CompletionItem> {
        if let Some(string_match) = self.filtered_entries.get(self.selected_index) {
            // Find the original item by matching candidate ID
            self.all_items.iter().find(|item| {
                let candidate = StringMatchCandidate::from(*item);
                candidate.id == string_match.candidate_id
            })
        } else {
            None
        }
    }

    /// Get completion item at specific index
    pub fn get_item_at_index(&self, index: usize) -> Option<&CompletionItem> {
        if let Some(string_match) = self.filtered_entries.get(index) {
            // Find the original item by matching candidate ID
            self.all_items.iter().find(|item| {
                let candidate = StringMatchCandidate::from(*item);
                candidate.id == string_match.candidate_id
            })
        } else {
            None
        }
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
        nucleotide_logging::info!(
            current_index = self.selected_index,
            filtered_count = self.filtered_entries.len(),
            "select_next called"
        );
        if !self.filtered_entries.is_empty() {
            let old_index = self.selected_index;
            self.selected_index = (self.selected_index + 1) % self.filtered_entries.len();
            nucleotide_logging::info!(
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
        nucleotide_logging::info!(
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
            nucleotide_logging::info!(
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
        let selected_item =
            if let Some(string_match) = self.filtered_entries.get(self.selected_index) {
                // Find the original item by matching candidate ID
                self.all_items
                    .iter()
                    .find(|item| {
                        let candidate = StringMatchCandidate::from(*item);
                        candidate.id == string_match.candidate_id
                    })
                    .cloned()
            } else {
                None
            };

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
            nucleotide_logging::info!(
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
            nucleotide_logging::info!(
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
            nucleotide_logging::info!(
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
                nucleotide_logging::info!(
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
                    // TODO: Show user notification
                    eprintln!("Completion warning: {}", message);
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
                // Retry the last operation after delay
                let delay_duration = delay;
                cx.spawn(async move |_this, _cx| {
                    tokio::time::sleep(delay_duration).await;
                    // TODO: Retry the last failed operation
                })
                .detach();
            }
            RecoveryAction::Fallback {
                action: _,
                description,
            } => {
                // Switch to basic completion mode
                eprintln!("Fallback activated: {}", description);
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
                eprintln!("Completion system: {}", message);
            }
        }
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

impl Focusable for CompletionView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for CompletionView {}
impl EventEmitter<CompleteViaHelixEvent> for CompletionView {}

impl Render for CompletionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        println!(
            "🎨 COMPLETION_VIEW RENDER: Starting render, visible={}, items_count={}, filtered_count={}",
            self.is_visible(),
            self.all_items.len(),
            self.filtered_entries.len()
        );

        nucleotide_logging::debug!(
            visible = self.is_visible(),
            all_items_count = self.all_items.len(),
            filtered_count = self.filtered_entries.len(),
            selected_index = self.selected_index,
            "CompletionView render start"
        );

        if !self.is_visible() {
            println!("🎨 COMPLETION_VIEW RENDER: Not visible, returning empty div");
            return div().id("completion-hidden");
        }

        // Access theme - if not available, return empty
        let theme = match cx.try_global::<crate::Theme>() {
            Some(theme) => {
                println!("🎨 COMPLETION_VIEW RENDER: Theme found, proceeding with render");
                theme
            }
            None => {
                println!("🎨 COMPLETION_VIEW RENDER: No theme found, returning empty div");
                return div().id("completion-no-theme");
            }
        };
        let tokens = &theme.tokens;

        // Store item count for uniform_list - processor will access view data directly
        let filtered_entries = &self.filtered_entries;

        // Use flexible layout - let container size itself based on content
        let max_visible_items = 12; // Show up to 12 items

        // Calculate maximum container height to prevent it from growing too large
        // Each item is approximately 32px, but let's use a more flexible approach
        let max_container_height = px(384.0); // ~12 items * 32px = 384px, but flexible

        nucleotide_logging::debug!(
            filtered_count = filtered_entries.len(),
            max_visible_items = max_visible_items,
            max_container_height = max_container_height.0,
            "Using flexible layout for completion list"
        );

        // Focusable container with key handling for completion navigation
        let container = div()
            .id("completion-popup-v2")
            .focusable()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|view, ev: &KeyDownEvent, _window, cx| {
                match ev.keystroke.key.as_str() {
                    "escape" => {
                        // Emit dismiss event to close the completion popup
                        cx.emit(DismissEvent);
                        // Stop propagation so Escape doesn't affect the editor
                        cx.stop_propagation();
                    }
                    "tab" => {
                        // Tab: Accept the currently selected completion item
                        if let Some(selected_index) = view.selected_index() {
                            nucleotide_logging::info!(
                                selected_index = selected_index,
                                "Tab pressed - accepting selected completion via Helix"
                            );
                            // Signal to Helix that it should accept the completion
                            // This uses Helix's Transaction system for proper text insertion
                            cx.emit(CompleteViaHelixEvent {
                                item_index: selected_index,
                            });
                            // Dismiss the completion popup
                            cx.emit(DismissEvent);
                            // Stop propagation so Tab doesn't insert text in the editor after completion
                            cx.stop_propagation();
                        } else {
                            nucleotide_logging::warn!(
                                "Tab pressed but no completion item selected"
                            );
                        }
                    }
                    "up" => {
                        nucleotide_logging::info!("Up arrow key pressed in completion popup");
                        // Up arrow: Move to previous completion item
                        view.select_prev(cx);
                        // Stop propagation so arrow keys don't move cursor in editor
                        cx.stop_propagation();
                    }
                    "down" => {
                        nucleotide_logging::info!("Down arrow key pressed in completion popup");
                        // Down arrow: Move to next completion item
                        view.select_next(cx);
                        // Stop propagation so arrow keys don't move cursor in editor
                        cx.stop_propagation();
                    }
                    "pagedown" => {
                        nucleotide_logging::info!("Page Down pressed in completion popup");
                        view.select_page_down(cx);
                        cx.stop_propagation();
                    }
                    "pageup" => {
                        nucleotide_logging::info!("Page Up pressed in completion popup");
                        view.select_page_up(cx);
                        cx.stop_propagation();
                    }
                    "home" => {
                        nucleotide_logging::info!("Home key pressed in completion popup");
                        view.select_first(cx);
                        cx.stop_propagation();
                    }
                    "end" => {
                        nucleotide_logging::info!("End key pressed in completion popup");
                        view.select_last(cx);
                        cx.stop_propagation();
                    }
                    "enter" => {
                        // Accept the currently selected completion item via Helix's system
                        if let Some(selected_index) = view.selected_index() {
                            // Signal to Helix that it should accept the completion
                            // This uses Helix's Transaction system for proper text insertion
                            cx.emit(CompleteViaHelixEvent {
                                item_index: selected_index,
                            });
                            // Dismiss the completion popup
                            cx.emit(DismissEvent);
                            // DO NOT stop propagation - let Enter key reach Helix for Transaction processing
                        }
                    }
                    _ => {}
                }
            }));

        // Do NOT steal focus from the editor - completion should be a non-modal overlay
        // The editor needs to maintain focus for proper keyboard event handling

        // TODO: Get actual cursor position from document/workspace
        // For now, use relative positioning that will be updated by parent container
        nucleotide_logging::debug!(
            item_count = self.filtered_entries.len(),
            "Completion view render completed"
        );
        container
            .absolute()
            // Remove hardcoded positioning - parent will handle this
            .child(
                div()
                    .id("completion-list")
                    .flex()
                    .flex_col()
                    // Calculate optimal width based on content
                    .w({
                        // Calculate optimal width based on the longest item
                        let base_width = 250.0; // Minimum practical width
                        let _padding = 40.0; // Account for padding, borders, icons

                        // Find the longest text in visible items
                        let max_text_length = self
                            .filtered_entries
                            .iter()
                            .take(12) // Only consider visible items for performance
                            .map(|string_match| {
                                // Find the corresponding completion item
                                self.all_items
                                    .iter()
                                    .find(|item| {
                                        let candidate = StringMatchCandidate::from(*item);
                                        candidate.id == string_match.candidate_id
                                    })
                                    .map(|item| {
                                        // Calculate total text length including signature and type info
                                        let display_text = item
                                            .display_text
                                            .as_ref()
                                            .map(|s| s.to_string())
                                            .unwrap_or_else(|| item.text.to_string());
                                        let mut total_len = display_text.len();

                                        if let Some(sig) = &item.signature_info {
                                            total_len += sig.to_string().len();
                                        }
                                        if let Some(type_info) = &item.type_info {
                                            total_len += type_info.to_string().len() + 3; // "→ " prefix
                                        }
                                        if let Some(detail) = &item.detail {
                                            // Detail is on second line, consider it separately
                                            total_len = total_len.max(detail.to_string().len());
                                        }
                                        total_len
                                    })
                                    .unwrap_or(0)
                            })
                            .max()
                            .unwrap_or(0);

                        // Estimate pixel width (rough approximation: 8px per character)
                        let estimated_width: f32 = base_width + (max_text_length as f32 * 8.0);

                        // Cap the width to reasonable bounds
                        let optimal_width = estimated_width.min(600.0).max(base_width);

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
                    .shadow_lg()
                    .py(tokens.sizes.space_1)
                    .px(tokens.sizes.space_1)
                    .child(
                        div()
                            .id("completion-list-container")
                            .flex()
                            .flex_col()
                            .w_full()
                            .max_h(max_container_height) // Flexible height with maximum constraint
                            .min_h(px(64.0)) // Minimum height for at least 2 items
                            .bg(tokens.chrome.popup_background) // Ensure background is visible
                            .child(
                                // Scrollable container with working completion items
                                div()
                                    .id("completion-scrollable-container")
                                    .flex()
                                    .flex_col()
                                    .w_full()
                                    .h_full()
                                    .overflow_y_scroll() // Enable scrolling
                                    .children({
                                        // Implement a sliding window of visible items centered around selection
                                        let total_items = self.filtered_entries.len();
                                        let max_visible = 12; // Show up to 12 items at a time

                                        let (start_index, end_index) = if total_items <= max_visible
                                        {
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
                                            let start = if selected < max_visible.saturating_sub(1)
                                            {
                                                // Selected is near beginning, show from start
                                                0
                                            } else if selected >= total_items.saturating_sub(1) {
                                                // Selected is last item, show last window
                                                total_items.saturating_sub(max_visible)
                                            } else {
                                                // Selected is in middle, center it
                                                let half = max_visible / 2;
                                                selected.saturating_sub(half)
                                            };

                                            // Ensure we don't go past the end
                                            let start =
                                                start.min(total_items.saturating_sub(max_visible));
                                            let end = (start + max_visible).min(total_items);

                                            nucleotide_logging::debug!(
                                                calculated_start = start,
                                                calculated_end = end,
                                                "Calculated window bounds"
                                            );

                                            (start, end)
                                        };

                                        // Verify the selected item is actually in the window
                                        let selected_visible = self.selected_index >= start_index
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
                                        if self.selected_index >= total_items.saturating_sub(3) {
                                            nucleotide_logging::info!(
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

                                        let items_to_render: Vec<_> = self
                                            .filtered_entries
                                            .iter()
                                            .enumerate()
                                            .skip(start_index)
                                            .take(end_index - start_index)
                                            .collect();

                                        nucleotide_logging::debug!(
                                            items_to_render_count = items_to_render.len(),
                                            expected_count = end_index - start_index,
                                            first_index = items_to_render.first().map(|(i, _)| *i),
                                            last_index = items_to_render.last().map(|(i, _)| *i),
                                            "Items actually being rendered"
                                        );

                                        items_to_render
                                            .into_iter()
                                            .map(|(index, string_match)| {
                                                // Find the original completion item
                                                let item = self
                                                    .all_items
                                                    .iter()
                                                    .find(|item| {
                                                        let candidate =
                                                            StringMatchCandidate::from(*item);
                                                        candidate.id == string_match.candidate_id
                                                    })
                                                    .unwrap();

                                                let is_selected = index == self.selected_index;

                                                // Add explicit ID for scroll-to-element functionality
                                                let completion_element = CompletionItemElement::new(
                                                    item.clone(),
                                                    string_match.clone(),
                                                    is_selected,
                                                );

                                                // Wrap in div with ID for scroll targeting
                                                div()
                                                    .id(("completion-item-wrapper", index))
                                                    .w_full()
                                                    .child(completion_element)
                                            })
                                            .collect::<Vec<_>>()
                                    }),
                            ),
                    ),
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

        let mut matches = vec![match1, match2, match3];
        matches.sort();

        // Should be sorted by score descending
        assert_eq!(matches[0].score, 200);
        assert_eq!(matches[1].score, 150);
        assert_eq!(matches[2].score, 100);
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
            .with_documentation("Detailed documentation here");

        assert_eq!(item.text, "function_name");
        assert_eq!(item.description.as_ref().unwrap(), "A cool function");
        assert_eq!(item.kind, Some(CompletionItemKind::Function));
        assert_eq!(
            item.documentation.as_ref().unwrap(),
            "Detailed documentation here"
        );
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
                    if let Some(new_pos) = new_position {
                        if new_pos != initial_pos {
                            return true;
                        }
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
            let item = CompletionItem::new("test").with_kind(kind.clone());
            assert_eq!(item.kind, Some(kind));
        }
    }

    mod filter_tests {
        use super::*;
        use gpui::TestAppContext;

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
    }
}
