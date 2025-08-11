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
