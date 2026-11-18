// ABOUTME: UI domain events emitted by the application.
// ABOUTME: Contains only events with active producers and consumers.

#[derive(Debug, Clone)]
pub enum Event {
    SystemAppearanceChanged {
        appearance: SystemAppearance,
    },
    /// The pointer moved onto a link in rendered Markdown (`Some`) or off it (`None`).
    MarkdownLinkHovered {
        uri: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAppearance {
    Light,
    Dark,
    Auto,
}
