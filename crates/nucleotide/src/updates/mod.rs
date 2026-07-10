// ABOUTME: Application update orchestration and Velopack integration.
// ABOUTME: Keeps blocking update work away from GPUI and exposes reactive UI state.

mod backend;
mod controller;
mod dialog;
mod indicator;
mod model;

pub use controller::{UpdateController, UpdateControllerEvent, UpdateControllerHandle};
pub use dialog::UpdateDialog;
pub use indicator::UpdateIndicator;
pub use model::{AvailableUpdate, CheckOrigin, UpdateOperation, UpdateState};

use velopack::VelopackApp;

#[cfg(target_os = "windows")]
const WINDOWS_APP_USER_MODEL_ID: &str = "org.spiralpoint.nucleotide";

/// Run Velopack lifecycle hooks before any normal application initialization.
pub fn run_startup_hooks() {
    #[cfg(target_os = "windows")]
    let mut app = VelopackApp::build().set_app_user_model_id(WINDOWS_APP_USER_MODEL_ID);

    #[cfg(not(target_os = "windows"))]
    let mut app = VelopackApp::build();

    app.run();
}
