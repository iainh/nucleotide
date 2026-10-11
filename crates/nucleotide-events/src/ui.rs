// ABOUTME: UI domain events emitted by the application.
// ABOUTME: Contains only events with active producers and consumers.

#[derive(Debug, Clone)]
pub enum Event {
    SystemAppearanceChanged { appearance: SystemAppearance },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAppearance {
    Light,
    Dark,
    Auto,
}
