// ABOUTME: GPUI action definitions following Zed's idiomatic pattern
// ABOUTME: Centralizes all actions for proper event handling and key contexts

// Define action namespaces as modules
pub mod editor {
    use gpui::actions;

    actions!(
        editor,
        [
            MoveUp,
            MoveDown,
            MoveLeft,
            MoveRight,
            PageUp,
            PageDown,
            Confirm,
            Cancel,
            Escape,
            Copy,
            Paste,
            Undo,
            Redo,
            Save,
            SaveAs,
            OpenFile,
            OpenDirectory,
            CloseFile,
            Quit,
            IncreaseFontSize,
            DecreaseFontSize,
        ]
    );
}

pub mod picker {
    use gpui::actions;

    actions!(
        picker,
        [
            SelectNext,
            SelectPrev,
            SelectFirst,
            SelectLast,
            ConfirmSelection,
            DismissPicker,
            TogglePreview,
        ]
    );
}

pub mod prompt {
    use gpui::actions;

    actions!(
        prompt,
        [SubmitPrompt, CancelPrompt, HistoryNext, HistoryPrev,]
    );
}

pub mod completion {
    use gpui::actions;

    actions!(
        completion,
        [
            CompletionSelectNext,
            CompletionSelectPrev,
            CompletionSelectFirst,
            CompletionSelectLast,
            CompletionConfirm,
            CompletionDismiss,
        ]
    );
}

pub mod window {
    use gpui::actions;

    actions!(
        window,
        [Hide, HideOthers, ShowAll, Minimize, Zoom, ToggleFullScreen,]
    );
}

pub mod workspace {
    use gpui::actions;

    actions!(
        workspace,
        [
            NewFile,
            NewWindow,
            CloseWindow,
            SplitVertically,
            SplitHorizontally,
            ToggleSidebar,
            ShowCommandPalette,
            ShowFileFinder,
            ShowSymbolFinder,
            ShowBufferPicker,
        ]
    );
}

pub mod help {
    use gpui::actions;

    actions!(
        help,
        [
            About,
            ShowDocumentation,
            ShowKeyboardShortcuts,
            OpenTutorial,
        ]
    );
}

pub mod test {
    use gpui::actions;

    actions!(test, [TestPrompt, TestCompletion, TestPicker,]);
}

pub mod file_tree {
    use gpui::actions;

    actions!(
        file_tree,
        [
            SelectNext,
            SelectPrev,
            ExpandCollapse,
            OpenFile,
            FocusFileTree,
        ]
    );
}
