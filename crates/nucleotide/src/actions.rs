// ABOUTME: Action definitions for nucleotide-widgets components
// ABOUTME: Provides GPUI actions for picker, prompt, and file tree interactions

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
