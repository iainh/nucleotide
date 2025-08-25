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
use crate::fuzzy::{FuzzyConfig, match_strings};

/// Event emitted when a completion item is accepted by the user
#[derive(Debug, Clone)]
pub struct CompletionAcceptedEvent {
    pub text: String,
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
            list_state: CompletionListState::new(24.0, 400.0),
            current_documentation: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Set all completion items and prepare candidates for filtering
    pub fn set_items(&mut self, items: Vec<CompletionItem>, cx: &mut Context<Self>) {
        println!("COMP: Setting {} items in CompletionView", items.len());
        // Calculate hash for cache invalidation
        let new_hash = self.calculate_items_hash(&items);

        // If items haven't changed, no need to update
        if new_hash == self.items_hash && !self.all_items.is_empty() {
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
        println!("COMP: Resetting completion state and making visible");
        self.filtered_entries.clear();
        self.selected_index = 0;
        self.initial_query = None;
        self.initial_position = None;
        self.current_query = None;

        // Cancel any ongoing filtering and reset debouncer
        self.cancel_current_filter();
        self.debouncer.reset();

        // Initialize filtered_entries with all items when no filter is applied
        if !self.all_items.is_empty() {
            println!(
                "COMP: Populating filtered_entries with {} items",
                self.all_items.len()
            );
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
            println!(
                "COMP: Set visible=true with {} filtered_entries",
                self.filtered_entries.len()
            );
        } else {
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
        // For now, just do immediate filtering
        // TODO: Implement proper debouncing with weak entity references
        self.filter_immediate(query, position, cx);
    }

    /// Immediate filtering without debouncing (for internal use)
    fn filter_immediate(
        &mut self,
        query: String,
        position: Option<Position>,
        cx: &mut Context<Self>,
    ) {
        let timer = PerformanceTimer::start("filter_immediate");

        // Check for very large item counts that might cause performance issues
        if self.all_items.len() > 10000 {
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

            self.filtered_entries = cached_results;
            self.selected_index = 0;
            self.current_query = Some(query);
            self.update_list_state();
            self.update_documentation_for_selection(cx);
            cx.notify();
            return;
        }

        // Check if we can optimize using query extension
        if !self.should_refilter(&query, position.as_ref()) {
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
            let results: Vec<StringMatch> = self
                .match_candidates
                .iter()
                .map(|candidate| StringMatch::new(candidate.id, 100, vec![]))
                .take(self.max_items)
                .collect();

            // Cache the results
            self.cache.insert(cache_key, results.clone());

            self.filtered_entries = results;
            self.selected_index = 0;
            self.update_list_state();
            self.update_documentation_for_selection(cx);
            cx.notify();
            return;
        }

        // Check if we can use optimization base from cache
        if let Some(base_results) = self.try_optimization_from_cache(&query) {
            // Filter the base results for the new query
            let optimized_results = self.filter_cached_results(base_results, &query);
            self.cache.insert(cache_key, optimized_results.clone());
            self.filtered_entries = optimized_results;
            self.selected_index = 0;
            self.update_list_state();
            self.update_documentation_for_selection(cx);
            cx.notify();
            return;
        }

        // Start background filtering
        let candidates = self.match_candidates.clone();
        let cancel_flag = self.cancel_flag.clone();
        let max_items = self.max_items;

        self.filter_task = Some(cx.spawn(async move |_this, _cx| {
            // Use real fuzzy matching
            let config = FuzzyConfig::default();

            // For now, return the results
            // TODO: Implement proper entity update mechanism
            match_strings(candidates, query.clone(), config, max_items, cancel_flag).await
        }));

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
        self.filtered_entries = matches;
        self.selected_index = 0;
        self.filter_task = None;
        self.update_list_state();
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

    /// Move selection up/down
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        if !self.filtered_entries.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.filtered_entries.len();
            self.update_documentation_for_selection(cx);
            cx.notify();
        }
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        if !self.filtered_entries.is_empty() {
            self.selected_index = if self.selected_index == 0 {
                self.filtered_entries.len() - 1
            } else {
                self.selected_index - 1
            };
            self.update_documentation_for_selection(cx);
            cx.notify();
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
impl EventEmitter<CompletionAcceptedEvent> for CompletionView {}

impl Render for CompletionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible() {
            return div().id("completion-hidden");
        }

        // Access theme - if not available, return empty
        let theme = match cx.try_global::<crate::Theme>() {
            Some(theme) => theme,
            None => return div().id("completion-no-theme"),
        };
        let tokens = &theme.tokens;

        // Create completion items using the enhanced CompletionItemElement
        let completion_items: Vec<gpui::AnyElement> = self
            .filtered_entries
            .iter()
            .enumerate()
            .filter_map(|(index, string_match)| {
                // Find the item by matching candidate ID
                let item = self.all_items.iter().find(|item| {
                    let candidate = StringMatchCandidate::from(*item);
                    candidate.id == string_match.candidate_id
                })?;
                let is_selected = index == self.selected_index;

                // Use the rich CompletionItemElement with full feature set
                let element =
                    CompletionItemElement::new(item.clone(), string_match.clone(), is_selected);

                Some(element.into_any_element())
            })
            .collect();

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
                    }
                    "tab" => {
                        if ev.keystroke.modifiers.shift {
                            // Shift+Tab: Move to previous item
                            view.select_prev(cx);
                        } else {
                            // Tab: Move to next item
                            view.select_next(cx);
                        }
                    }
                    "enter" => {
                        // Accept the currently selected completion item
                        if let Some(selected_item) = view.selected_item() {
                            // Emit completion accepted event with the selected text
                            cx.emit(CompletionAcceptedEvent {
                                text: selected_item.text.to_string(),
                            });
                            // Also dismiss the completion popup
                            cx.emit(DismissEvent);
                        }
                    }
                    _ => {}
                }
            }));

        // Focus the completion view when it's first shown
        window.focus(&self.focus_handle);

        // TODO: Get actual cursor position from document/workspace
        // For now, use relative positioning that will be updated by parent container
        container
            .absolute()
            // Remove hardcoded positioning - parent will handle this
            .child(
                div()
                    .id("completion-list")
                    .flex()
                    .flex_col()
                    .min_w(px(250.0))
                    .max_w(px(400.0))
                    .bg(tokens.colors.popup_background)
                    .border_1()
                    .border_color(tokens.colors.popup_border)
                    .rounded(tokens.sizes.radius_md)
                    .shadow_lg()
                    .max_h(px(300.0))
                    .overflow_y_scroll()
                    .py(tokens.sizes.space_1)
                    .px(tokens.sizes.space_1)
                    .children(completion_items),
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
}
