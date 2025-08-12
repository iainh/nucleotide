// ABOUTME: UI action definitions for picker and other components
// ABOUTME: Defines GPUI actions that can be triggered by user input

use gpui::actions;

pub mod picker {
    use super::*;

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
    use super::*;

    actions!(
        completion,
        [
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
    use super::*;

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
    use super::*;

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
    use super::*;

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
    use super::*;

    actions!(help, [About, OpenTutorial,]);
}

pub mod workspace {
    use super::*;

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
    use super::*;

    actions!(window, [Hide, HideOthers, ShowAll, Minimize, Zoom,]);
}

pub mod test {
    use super::*;

    actions!(test, [TestPrompt, TestCompletion,]);
}

// Common editor navigation actions
pub mod common {
    use super::*;

    actions!(
        common,
        [MoveUp, MoveDown, MoveLeft, MoveRight, Confirm, Cancel,]
    );
}
