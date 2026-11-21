//! Window geometry and workspace chrome state persisted across restarts.
//!
//! The state lives in one small JSON file. The last window to change its geometry or chrome
//! writes it, and the next launch restores it, clamped to a display that is still connected.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use gpui::{Bounds, Pixels, WindowBounds, point, px, size};
use nucleotide_logging::{debug, warn};
use serde::{Deserialize, Serialize};

/// Overrides the directory that holds the state file.
const STATE_DIR_ENV: &str = "NUCLEOTIDE_STATE_DIR";
/// Set on a process started by `NewWindow` so it opens as an additional window.
pub const NEW_WINDOW_ENV: &str = "NUCLEOTIDE_NEW_WINDOW";
const STATE_FILE_NAME: &str = "window-state.json";

/// Bounds used when nothing has been saved yet.
const DEFAULT_BOUNDS: SavedBounds = SavedBounds {
    x: 100.0,
    y: 100.0,
    width: 1200.0,
    height: 800.0,
};
/// Matches the minimum size passed to GPUI when the window opens.
pub const MIN_WINDOW_WIDTH: f32 = 400.0;
pub const MIN_WINDOW_HEIGHT: f32 = 300.0;
/// Offset of a window opened by `NewWindow` from the saved window, so it does not cover it.
const CASCADE_OFFSET: f32 = 32.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub window: Option<SavedWindow>,
    pub chrome: ChromeState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWindow {
    pub mode: WindowMode,
    /// Windowed bounds, or the restore bounds of a maximized or fullscreen window, in the
    /// coordinates GPUI uses for the display it is on.
    pub bounds: SavedBounds,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_uuid: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    #[default]
    Windowed,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChromeState {
    pub show_file_tree: bool,
    /// Width the user dragged the file tree to; `None` keeps the default width.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_tree_width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_sidebar_width: Option<f32>,
}

impl Default for ChromeState {
    fn default() -> Self {
        Self {
            show_file_tree: true,
            file_tree_width: None,
            doc_sidebar_width: None,
        }
    }
}

impl SavedBounds {
    fn from_gpui(bounds: Bounds<Pixels>) -> Self {
        Self {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
        }
    }

    fn to_gpui(self) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(self.x), px(self.y)),
            size: size(px(self.width), px(self.height)),
        }
    }

    fn is_finite(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|value| value.is_finite())
    }
}

impl SavedWindow {
    pub fn from_gpui(bounds: WindowBounds, display_uuid: Option<String>) -> Self {
        let (mode, bounds) = match bounds {
            WindowBounds::Windowed(bounds) => (WindowMode::Windowed, bounds),
            WindowBounds::Maximized(bounds) => (WindowMode::Maximized, bounds),
            WindowBounds::Fullscreen(bounds) => (WindowMode::Fullscreen, bounds),
        };
        Self {
            mode,
            bounds: SavedBounds::from_gpui(bounds),
            display_uuid,
        }
    }
}

/// A connected display, as GPUI reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayArea {
    pub uuid: Option<String>,
    pub bounds: Bounds<Pixels>,
}

/// Where to open a window: the index of the display in the list passed to
/// [`resolve_window_placement`], if any, and the bounds to open with.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowPlacement {
    pub display: Option<usize>,
    pub bounds: WindowBounds,
}

/// Chooses the bounds and display for a new window from the saved window.
///
/// Prefers the display the window was saved on, then the display it overlaps most, then the
/// first display. The bounds are shrunk to fit that display and moved fully onto it, so a window
/// saved on a disconnected or rearranged monitor still opens visibly. `cascade` offsets the window
/// from the saved one and never opens it fullscreen, for windows opened by `NewWindow`.
pub fn resolve_window_placement(
    saved: Option<&SavedWindow>,
    displays: &[DisplayArea],
    cascade: bool,
) -> WindowPlacement {
    let saved = saved.filter(|saved| saved.bounds.is_finite());
    let mut mode = saved.map_or(WindowMode::Windowed, |saved| saved.mode);
    let mut bounds = saved.map_or(DEFAULT_BOUNDS, |saved| saved.bounds);

    if cascade {
        bounds.x += CASCADE_OFFSET;
        bounds.y += CASCADE_OFFSET;
        if mode == WindowMode::Fullscreen {
            mode = WindowMode::Windowed;
        }
    }

    let display = saved
        .and_then(|saved| saved.display_uuid.as_deref())
        .and_then(|uuid| {
            displays
                .iter()
                .position(|display| display.uuid.as_deref() == Some(uuid))
        })
        .or_else(|| display_with_largest_overlap(bounds.to_gpui(), displays))
        .or_else(|| (!displays.is_empty()).then_some(0));

    let bounds = match display {
        Some(index) => clamp_to_display(bounds, displays[index].bounds),
        None => bounds,
    }
    .to_gpui();

    WindowPlacement {
        display,
        bounds: match mode {
            WindowMode::Windowed => WindowBounds::Windowed(bounds),
            WindowMode::Maximized => WindowBounds::Maximized(bounds),
            WindowMode::Fullscreen => WindowBounds::Fullscreen(bounds),
        },
    }
}

fn display_with_largest_overlap(bounds: Bounds<Pixels>, displays: &[DisplayArea]) -> Option<usize> {
    displays
        .iter()
        .enumerate()
        .filter_map(|(index, display)| {
            let overlap = bounds.intersect(&display.bounds);
            let area = f32::from(overlap.size.width) * f32::from(overlap.size.height);
            (area > 0.0).then_some((index, area))
        })
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .map(|(index, _)| index)
}

fn clamp_to_display(bounds: SavedBounds, display: Bounds<Pixels>) -> SavedBounds {
    let display_x = f32::from(display.origin.x);
    let display_y = f32::from(display.origin.y);
    let display_width = f32::from(display.size.width);
    let display_height = f32::from(display.size.height);

    let width = bounds.width.max(MIN_WINDOW_WIDTH).min(display_width);
    let height = bounds.height.max(MIN_WINDOW_HEIGHT).min(display_height);
    let x = bounds.x.clamp(display_x, display_x + display_width - width);
    let y = bounds
        .y
        .clamp(display_y, display_y + display_height - height);

    SavedBounds {
        x,
        y,
        width,
        height,
    }
}

/// The state file, or `None` when the platform has no state or data directory.
pub fn state_file_path() -> Option<PathBuf> {
    if let Some(directory) = std::env::var_os(STATE_DIR_ENV) {
        return Some(PathBuf::from(directory).join(STATE_FILE_NAME));
    }

    #[cfg(target_os = "windows")]
    let base = dirs::data_local_dir().map(|dir| dir.join("Spiralpoint").join("Nucleotide"));
    #[cfg(not(target_os = "windows"))]
    let base = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join("nucleotide"));

    base.map(|dir| dir.join(STATE_FILE_NAME))
}

/// Loads the saved state; a missing or unreadable file gives the defaults.
pub fn load() -> WindowState {
    state_file_path()
        .map(|path| load_from(&path))
        .unwrap_or_default()
}

pub fn load_from(path: &Path) -> WindowState {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return WindowState::default();
        }
        Err(error) => {
            warn!(path = %path.display(), error = %error, "Failed to read window state");
            return WindowState::default();
        }
    };

    serde_json::from_str(&contents).unwrap_or_else(|error| {
        warn!(path = %path.display(), error = %error, "Ignoring invalid window state");
        WindowState::default()
    })
}

pub fn save(state: &WindowState) {
    let Some(path) = state_file_path() else {
        return;
    };
    if let Err(error) = save_to(&path, state) {
        warn!(path = %path.display(), error = %error, "Failed to save window state");
    } else {
        debug!(path = %path.display(), "Saved window state");
    }
}

/// Writes the state through a temporary file so a crash never leaves a truncated file behind.
pub fn save_to(path: &Path, state: &WindowState) -> std::io::Result<()> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(directory)?;
    // Every window is its own process, so each writes through its own temporary file.
    let temporary = directory.join(format!(".{STATE_FILE_NAME}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        serde_json::to_writer_pretty(&mut file, state)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Starts another Nucleotide process that opens one empty window.
///
/// Each window owns a whole Helix editor, so a new window is a new process. It starts in the
/// home directory so it does not reopen the current project, and [`NEW_WINDOW_ENV`] makes it
/// cascade from the saved window and skip single-instance forwarding on Windows.
pub fn spawn_new_window_process() -> std::io::Result<()> {
    let executable = std::env::current_exe()?;
    let mut command = std::process::Command::new(executable);
    command
        .env(NEW_WINDOW_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(home) = dirs::home_dir() {
        command.current_dir(home);
    }

    let mut child = command.spawn()?;
    // Reap the child when it exits so it does not linger as a zombie on Unix.
    std::thread::Builder::new()
        .name("NewWindowProcess".to_string())
        .spawn(move || {
            let _ = child.wait();
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(uuid: &str, x: f32, y: f32, width: f32, height: f32) -> DisplayArea {
        DisplayArea {
            uuid: Some(uuid.to_string()),
            bounds: Bounds {
                origin: point(px(x), px(y)),
                size: size(px(width), px(height)),
            },
        }
    }

    fn saved(mode: WindowMode, bounds: SavedBounds, uuid: Option<&str>) -> SavedWindow {
        SavedWindow {
            mode,
            bounds,
            display_uuid: uuid.map(str::to_string),
        }
    }

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> SavedBounds {
        SavedBounds {
            x,
            y,
            width,
            height,
        }
    }

    fn placed(placement: &WindowPlacement) -> SavedBounds {
        SavedBounds::from_gpui(placement.bounds.get_bounds())
    }

    #[test]
    fn restores_window_on_its_saved_display() {
        let displays = [
            display("left", 0.0, 0.0, 1920.0, 1080.0),
            display("right", 1920.0, 0.0, 2560.0, 1440.0),
        ];
        let window = saved(
            WindowMode::Maximized,
            bounds(2000.0, 50.0, 1400.0, 900.0),
            Some("right"),
        );

        let placement = resolve_window_placement(Some(&window), &displays, false);

        assert_eq!(placement.display, Some(1));
        assert!(matches!(placement.bounds, WindowBounds::Maximized(_)));
        assert_eq!(placed(&placement), bounds(2000.0, 50.0, 1400.0, 900.0));
    }

    #[test]
    fn window_from_disconnected_display_moves_onto_overlapping_display() {
        // The saved display is gone and the window mostly overlaps the second display.
        let displays = [
            display("left", 0.0, 0.0, 1920.0, 1080.0),
            display("right", 1920.0, 0.0, 1280.0, 800.0),
        ];
        let window = saved(
            WindowMode::Windowed,
            bounds(2400.0, 500.0, 1400.0, 900.0),
            Some("unplugged"),
        );

        let placement = resolve_window_placement(Some(&window), &displays, false);

        assert_eq!(placement.display, Some(1));
        // Shrunk to the 1280x800 display and moved so its right and bottom edges are on it.
        assert_eq!(placed(&placement), bounds(1920.0, 0.0, 1280.0, 800.0));
    }

    #[test]
    fn window_off_every_display_moves_onto_first_display() {
        let displays = [display("only", 0.0, 0.0, 1440.0, 900.0)];
        let window = saved(
            WindowMode::Windowed,
            bounds(-3000.0, 2000.0, 800.0, 600.0),
            None,
        );

        let placement = resolve_window_placement(Some(&window), &displays, false);

        assert_eq!(placement.display, Some(0));
        assert_eq!(placed(&placement), bounds(0.0, 300.0, 800.0, 600.0));
    }

    #[test]
    fn tiny_saved_window_grows_to_minimum_size() {
        let displays = [display("only", 0.0, 0.0, 1440.0, 900.0)];
        let window = saved(WindowMode::Windowed, bounds(10.0, 20.0, 50.0, 40.0), None);

        let placement = resolve_window_placement(Some(&window), &displays, false);

        assert_eq!(
            placed(&placement),
            bounds(10.0, 20.0, MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT)
        );
    }

    #[test]
    fn first_launch_uses_default_bounds_clamped_to_display() {
        let displays = [display("small", 0.0, 0.0, 1024.0, 768.0)];

        let placement = resolve_window_placement(None, &displays, false);

        assert!(matches!(placement.bounds, WindowBounds::Windowed(_)));
        assert_eq!(placed(&placement), bounds(0.0, 0.0, 1024.0, 768.0));
    }

    #[test]
    fn new_window_cascades_and_leaves_fullscreen() {
        let displays = [display("only", 0.0, 0.0, 1920.0, 1080.0)];
        let window = saved(
            WindowMode::Fullscreen,
            bounds(100.0, 80.0, 1200.0, 800.0),
            Some("only"),
        );

        let placement = resolve_window_placement(Some(&window), &displays, true);

        assert!(matches!(placement.bounds, WindowBounds::Windowed(_)));
        assert_eq!(
            placed(&placement),
            bounds(100.0 + CASCADE_OFFSET, 80.0 + CASCADE_OFFSET, 1200.0, 800.0)
        );
    }

    #[test]
    fn non_finite_saved_bounds_fall_back_to_defaults() {
        let displays = [display("only", 0.0, 0.0, 1920.0, 1080.0)];
        let window = saved(
            WindowMode::Maximized,
            bounds(f32::NAN, 0.0, 800.0, 600.0),
            None,
        );

        let placement = resolve_window_placement(Some(&window), &displays, false);

        assert!(matches!(placement.bounds, WindowBounds::Windowed(_)));
        assert_eq!(placed(&placement), DEFAULT_BOUNDS);
    }

    #[test]
    fn state_round_trips_and_bad_files_give_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(STATE_FILE_NAME);
        assert_eq!(load_from(&path), WindowState::default());

        let state = WindowState {
            window: Some(saved(
                WindowMode::Maximized,
                bounds(1.0, 2.0, 1300.0, 900.0),
                Some("abc"),
            )),
            chrome: ChromeState {
                show_file_tree: false,
                file_tree_width: Some(312.0),
                doc_sidebar_width: Some(420.0),
            },
        };
        save_to(&path, &state).unwrap();
        assert_eq!(load_from(&path), state);

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_from(&path), WindowState::default());

        // Older files without chrome keep the file tree visible.
        std::fs::write(&path, r#"{"window":null}"#).unwrap();
        assert!(load_from(&path).chrome.show_file_tree);
    }
}
