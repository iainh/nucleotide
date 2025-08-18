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
            OpenFile,
            RefreshTree,
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
            Save,
            SaveAs,
            CloseFile,
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

    actions!(help, [About, OpenTutorial,]);
}

pub mod workspace {
    use super::actions;

    actions!(
        workspace,
        [
            ShowBufferPicker,
            ToggleFileTree,
            ShowFileFinder,
            NewFile,
            NewWindow,
            ShowCommandPalette,
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
