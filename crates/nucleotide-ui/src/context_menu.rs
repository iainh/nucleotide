// ABOUTME: Shared context-menu open-state controller
// ABOUTME: Popup menu rendering lives in menu::PopupMenu

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContextMenuController {
    open: bool,
    position: (f32, f32),
}

impl ContextMenuController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn position(&self) -> (f32, f32) {
        self.position
    }

    pub fn open_at(&mut self, position: (f32, f32)) {
        self.open = true;
        self.position = position;
    }

    pub fn close(&mut self) -> bool {
        if !self.open {
            return false;
        }

        self.open = false;
        true
    }
}

impl Default for ContextMenuController {
    fn default() -> Self {
        Self {
            open: false,
            position: (0.0, 0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_opens_at_position() {
        let mut controller = ContextMenuController::new();

        assert!(!controller.is_open());
        controller.open_at((12.0, 24.0));

        assert!(controller.is_open());
        assert_eq!(controller.position(), (12.0, 24.0));
    }

    #[test]
    fn controller_close_reports_changes() {
        let mut controller = ContextMenuController::new();

        assert!(!controller.close());

        controller.open_at((1.0, 2.0));
        assert!(controller.close());
        assert!(!controller.is_open());
        assert!(!controller.close());
    }
}
