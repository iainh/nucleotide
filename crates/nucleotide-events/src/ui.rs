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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_appearance_events_cover_all_modes() {
        for appearance in [
            SystemAppearance::Light,
            SystemAppearance::Dark,
            SystemAppearance::Auto,
        ] {
            let Event::SystemAppearanceChanged {
                appearance: event_appearance,
            } = Event::SystemAppearanceChanged { appearance };
            assert_eq!(event_appearance, appearance);
        }
    }
}
