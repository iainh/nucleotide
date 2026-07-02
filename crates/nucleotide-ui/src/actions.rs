// ABOUTME: UI action definitions for picker and other components
// ABOUTME: Defines GPUI actions that can be triggered by user input

use gpui::actions;

pub mod picker {
    use super::actions;

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

pub mod completion {
    use super::actions;

    actions!(
        completion,
        [
            TriggerCompletion,
            CompletionSelectNext,
            CompletionSelectPrev,
            CompletionSelectFirst,
            CompletionSelectLast,
            CompletionConfirm,
            CompletionCancel,
            CompletionDismiss,
        ]
    );
}

pub mod prompt {
    use super::actions;

    actions!(
        prompt,
        [
            Confirm,
            Cancel,
            DeleteWord,
            DeleteChar,
            MoveToStart,
            MoveToEnd,
            MoveCursorLeft,
            MoveCursorRight,
            NextCompletion,
            PrevCompletion,
        ]
    );
}

pub mod file_tree {
    use super::actions;

    actions!(
        file_tree,
        [
            ToggleExpanded,
            SelectNext,
            SelectPrev,
            StartSearch,
            ClearSearch,
            SelectNextSearchMatch,
            SelectPrevSearchMatch,
            OpenFile,
            RefreshTree,
            // Context menu and common file ops
            OpenContextMenu,
            Rename,
            Delete,
            NewFile,
            NewFolder,
            Duplicate,
            CopyPath,
            CopyRelativePath,
            RevealInOs,
        ]
    );
}

pub mod editor {
    use super::actions;

    actions!(
        editor,
        [
            Quit,
            OpenFile,
            OpenDirectory,
            OpenRemote,
            OpenSettings,
            ReloadConfiguration,
            Save,
            SaveAs,
            CloseFile,
            RevertCurrentChange,
            Undo,
            Redo,
            Copy,
            Paste,
            IncreaseFontSize,
            DecreaseFontSize,
        ]
    );
}

pub mod help {
    use super::actions;

    actions!(help, [About, OpenTutorial, ThemeDebug,]);
}

pub mod workspace {
    use super::actions;

    actions!(
        workspace,
        [
            ShowBufferPicker,
            ShowCodeActions,
            ToggleFileTree,
            ToggleDocumentation,
            ToggleTerminal,
            ShowFileFinder,
            NewFile,
            NewWindow,
            ShowCommandPrompt,
            ShowRunnables,
            RunNearest,
            RunFileTests,
            RunLast,
            SplitPaneRight,
            SplitPaneLeft,
            SplitPaneUp,
            SplitPaneDown,
            UnpinAllTabs,
            TogglePreviewTab,
        ]
    );
}

pub mod window {
    use super::actions;

    actions!(window, [Hide, HideOthers, ShowAll, Minimize, Zoom,]);
}

pub mod test {
    use super::actions;

    actions!(test, [TestPrompt, TestCompletion,]);
}

// Common editor navigation actions
pub mod common {
    use super::actions;

    actions!(
        common,
        [MoveUp, MoveDown, MoveLeft, MoveRight, Confirm, Cancel,]
    );
}

pub mod menu {
    use super::actions;

    actions!(
        menu,
        [
            SelectUp,
            SelectDown,
            SelectLeft,
            SelectRight,
            Confirm,
            Cancel,
        ]
    );
}
