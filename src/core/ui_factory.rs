// ABOUTME: UI component factory functionality extracted from Application
// ABOUTME: Creates native GPUI components for pickers, prompts, and other UI elements

use std::path::PathBuf;
use std::sync::Arc;
use ignore::WalkBuilder;
use helix_view::Editor;
use crate::picker_view::PickerItem;

/// Factory for creating UI components
pub struct UiFactory<'a> {
    editor: &'a Editor,
}

impl<'a> UiFactory<'a> {
    pub fn new(editor: &'a Editor) -> Self {
        Self { editor }
    }

    /// Create a native file picker with items from the workspace
    pub fn create_file_picker(&self) -> crate::picker::Picker {
        let items = self.create_file_picker_items();
        
        crate::picker::Picker::native(
            "Open File",
            items,
            |_index| {
                // File selection logic would be handled by the caller
                println!("File selected at index: {_index}");
            },
        )
    }

    /// Create sample native prompt for search
    pub fn create_search_prompt(&self) -> crate::prompt::Prompt {
        crate::prompt::Prompt::native(
            "Search:",
            "",
            |input| {
                println!("Search submitted with input: '{input}'");
            }
        ).with_cancel(|| {
            println!("Search cancelled");
        })
    }

    /// Create command palette prompt
    pub fn create_command_prompt(&self) -> crate::prompt::Prompt {
        crate::prompt::Prompt::native(
            ":",
            "",
            |input| {
                println!("Command submitted: '{input}'");
            }
        ).with_cancel(|| {
            println!("Command cancelled");
        })
    }

    /// Create sample completion items for testing
    pub fn create_sample_completion_items(&self) -> Vec<crate::completion::CompletionItem> {
        use crate::completion::{CompletionItem, CompletionItemKind};
        
        vec![
            CompletionItem::new("println!", CompletionItemKind::Snippet)
                .with_detail("macro")
                .with_documentation("Prints to the standard output, with a newline."),
            CompletionItem::new("String", CompletionItemKind::Struct)
                .with_detail("std::string::String")
                .with_documentation("A UTF-8 encoded, growable string."),
            CompletionItem::new("Vec", CompletionItemKind::Struct)
                .with_detail("std::vec::Vec<T>")
                .with_documentation("A contiguous growable array type."),
            CompletionItem::new("HashMap", CompletionItemKind::Struct)
                .with_detail("std::collections::HashMap<K, V>")
                .with_documentation("A hash map implementation."),
            CompletionItem::new("println", CompletionItemKind::Function)
                .with_detail("fn println(&str)")
                .with_documentation("Print to stdout with newline"),
            CompletionItem::new("print", CompletionItemKind::Function)
                .with_detail("fn print(&str)")
                .with_documentation("Print to stdout without newline"),
            CompletionItem::new("format", CompletionItemKind::Function)
                .with_detail("fn format(&str, ...) -> String")
                .with_documentation("Create a formatted string"),
        ]
    }

    /// Create picker items from files in the workspace
    fn create_file_picker_items(&self) -> Vec<crate::picker_view::PickerItem> {
        
        let mut items = Vec::new();
        
        // Find workspace root
        let workspace_root = self.find_workspace_root();
        
        // Use WalkBuilder to walk all files
        let mut walk_builder = WalkBuilder::new(&workspace_root);
        walk_builder
            .hidden(false)  // Show hidden files (configurable)
            .follow_links(true)
            .git_ignore(true)  // Respect .gitignore
            .git_global(true)  // Respect global .gitignore
            .git_exclude(true) // Respect .git/info/exclude
            .sort_by_file_name(|a, b| a.cmp(b))
            .filter_entry(|entry| {
                // Filter out VCS directories and common build directories
                let path = entry.path();
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                
                // Skip common VCS and build directories
                if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    !matches!(name, ".git" | ".svn" | ".hg" | ".jj" | "target" | "node_modules" | ".idea" | ".vscode")
                } else {
                    true
                }
            })
            .max_depth(Some(10)); // Limit depth to prevent excessive traversal
        
        for entry in walk_builder.build() {
            if let Ok(entry) = entry {
                let path = entry.path().to_path_buf();
                
                // Skip directories in the picker items
                if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    continue;
                }
                
                // Get relative path for display
                let relative_path_str = path.strip_prefix(&workspace_root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                
                let label = relative_path_str.clone().into();
                
                // Add directory as sublabel if it has one
                let sublabel = path.strip_prefix(&workspace_root)
                    .unwrap_or(&path)
                    .parent()
                    .and_then(|p| if p.as_os_str().is_empty() { None } else { Some(p) })
                    .map(|p| p.to_string_lossy().to_string().into());
                
                items.push(PickerItem {
                    label,
                    sublabel,
                    data: Arc::new(path),
                });
            }
        }
        
        items
    }

    /// Find the workspace root directory
    fn find_workspace_root(&self) -> PathBuf {
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        
        // Walk up the directory tree looking for VCS directories
        for ancestor in current_dir.ancestors() {
            if ancestor.join(".git").exists()
                || ancestor.join(".svn").exists()
                || ancestor.join(".hg").exists()
                || ancestor.join(".jj").exists()
                || ancestor.join(".helix").exists()
            {
                return ancestor.to_path_buf();
            }
        }
        
        // If no VCS directory found, use current directory
        current_dir
    }

    /// Create a picker for buffer selection
    pub fn create_buffer_picker(&self) -> crate::picker::Picker {
        
        let mut items = Vec::new();
        
        // Get all open documents
        for doc in self.editor.documents() {
            let doc_id = doc.id();
            let path: String = doc.path()
                .map(|p| p.to_string_lossy().into())
                .unwrap_or_else(|| format!("[scratch.{}]", doc_id).into());
            
            let modified = if doc.is_modified() { " [+]" } else { "" };
            let label = format!("{}{}", path, modified).into();
            
            items.push(PickerItem {
                label,
                sublabel: None,
                data: Arc::new(doc_id),
            });
        }
        
        crate::picker::Picker::native(
            "Switch Buffer",
            items,
            |_index| {
                println!("Buffer selected at index: {_index}");
            },
        )
    }

    /// Create a symbol picker for the current document
    pub fn create_symbol_picker(&self, doc_id: helix_view::DocumentId) -> Option<crate::picker::Picker> {
        
        let doc = self.editor.document(doc_id)?;
        let _syntax = doc.syntax()?;
        
        // This is a simplified version - real implementation would use tree-sitter queries
        let items = Vec::new();
        
        // For now, just return empty picker
        Some(crate::picker::Picker::native(
            "Go to Symbol",
            items,
            |_index| {
                println!("Symbol selected at index: {_index}");
            },
        ))
    }
}