use collections::HashMap;
use wayland_client::{Proxy, protocol::wl_surface};

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) enum SerialKind {
    DataDevice,
    InputMethod,
    MouseEnter,
    MousePress,
    KeyPress,
}

#[derive(Debug)]
struct SerialData {
    serial: u32,
}

impl SerialData {
    fn new(value: u32) -> Self {
        Self { serial: value }
    }
}

/// The user interaction an activation token request must reference.
#[derive(Clone, Debug)]
pub(crate) struct ActivationContext {
    pub(crate) serial: u32,
    pub(crate) requesting_surface: wl_surface::WlSurface,
}

#[derive(Debug)]
struct Press {
    serial: u32,
    focused_surface: Option<wl_surface::WlSurface>,
}

#[derive(Debug)]
/// Helper for tracking of different serial kinds.
pub(crate) struct SerialTracker {
    serials: HashMap<SerialKind, SerialData>,
    latest_press: Option<Press>,
}

impl SerialTracker {
    pub fn new() -> Self {
        Self {
            serials: HashMap::default(),
            latest_press: None,
        }
    }

    pub fn update(&mut self, kind: SerialKind, value: u32) {
        self.update_with_focused_surface(kind, value, None);
    }

    pub fn update_with_focused_surface(
        &mut self,
        kind: SerialKind,
        value: u32,
        focused_surface: Option<wl_surface::WlSurface>,
    ) {
        if matches!(kind, SerialKind::KeyPress | SerialKind::MousePress) {
            self.latest_press = Some(Press {
                serial: value,
                focused_surface,
            });
        }
        self.serials.insert(kind, SerialData::new(value));
    }

    /// Returns the latest tracked serial of the provided [`SerialKind`]
    ///
    /// Will return 0 if not tracked.
    pub fn get(&self, kind: SerialKind) -> u32 {
        self.serials
            .get(&kind)
            .map(|serial_data| serial_data.serial)
            .unwrap_or(0)
    }

    /// Selection requests reference the latest key or pointer press, not an
    /// unrelated protocol serial. Arrival order also handles serial wraparound.
    pub fn selection_serial(&self) -> Option<u32> {
        self.latest_press.as_ref().map(|press| press.serial)
    }

    pub fn activation_context(&self) -> Option<ActivationContext> {
        let press = self.latest_press.as_ref()?;
        let surface = press
            .focused_surface
            .as_ref()
            .filter(|surface| surface.is_alive())?;
        Some(ActivationContext {
            serial: press.serial,
            requesting_surface: surface.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_tracks_press_arrival_order_not_largest_protocol_serial() {
        let mut tracker = SerialTracker::new();
        tracker.update(SerialKind::MouseEnter, 10);
        assert_eq!(tracker.selection_serial(), None);
        tracker.update(SerialKind::KeyPress, u32::MAX);
        tracker.update(SerialKind::DataDevice, 100);
        assert_eq!(tracker.selection_serial(), Some(u32::MAX));
        tracker.update(SerialKind::MousePress, 0);
        tracker.update(SerialKind::InputMethod, 200);
        assert_eq!(tracker.selection_serial(), Some(0));
        assert!(tracker.activation_context().is_none());
    }
}
