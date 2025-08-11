// ABOUTME: Action definitions for nucleotide-widgets components
// ABOUTME: Provides GPUI actions for picker, prompt, and file tree interactions

// Picker actions moved to nucleotide-ui
pub use nucleotide_ui::actions::picker;

pub mod prompt {
    use gpui::actions;

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
    use gpui::actions;

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
    use gpui::actions;

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
    use gpui::actions;

    actions!(help, [About, OpenTutorial,]);
}

pub mod workspace {
    use gpui::actions;

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
    use gpui::actions;

    actions!(window, [Hide, HideOthers, ShowAll, Minimize, Zoom,]);
}

pub mod test {
    use gpui::actions;

    actions!(test, [TestPrompt, TestCompletion,]);
}

// Completion actions moved to nucleotide-ui
pub use nucleotide_ui::actions::completion;

// Common editor navigation actions
use gpui::actions;
actions!(
    common,
    [MoveUp, MoveDown, MoveLeft, MoveRight, Confirm, Cancel,]
);
