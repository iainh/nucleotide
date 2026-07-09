use std::env;

use nucleotide_logging::{info, warn};
use velopack::{UpdateCheck, UpdateManager, VelopackApp, sources::AutoSource};

const DEFAULT_UPDATE_SOURCE: &str = "https://github.com/iainh/nucleotide";
const UPDATE_SOURCE_ENV: &str = "NUCLEOTIDE_UPDATE_SOURCE";
const DISABLE_UPDATES_ENV: &str = "NUCLEOTIDE_DISABLE_AUTO_UPDATE";
#[cfg(target_os = "windows")]
const WINDOWS_APP_USER_MODEL_ID: &str = "org.spiralpoint.nucleotide";

pub fn run_startup_hooks() {
    #[cfg(target_os = "windows")]
    let mut app = VelopackApp::build().set_app_user_model_id(WINDOWS_APP_USER_MODEL_ID);

    #[cfg(not(target_os = "windows"))]
    let mut app = VelopackApp::build();

    app.run();
}

pub fn spawn_background_update_check() {
    if updates_disabled() {
        info!(
            env = DISABLE_UPDATES_ENV,
            "Velopack automatic update checks are disabled"
        );
        return;
    }

    let source = update_source();
    if let Err(err) = std::thread::Builder::new()
        .name("nucleotide-velopack-updates".to_string())
        .spawn(move || check_and_stage_updates(&source))
    {
        warn!(error = %err, "Failed to spawn Velopack update check thread");
    }
}

fn updates_disabled() -> bool {
    env::var(DISABLE_UPDATES_ENV)
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

fn update_source() -> String {
    env::var(UPDATE_SOURCE_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_UPDATE_SOURCE.to_owned())
}

fn check_and_stage_updates(source: &str) {
    match try_check_and_stage_updates(source) {
        Ok(()) => {}
        Err(velopack::Error::NotInstalled(reason)) => {
            info!(
                reason = %reason,
                "Skipping Velopack update check because this build is not installed by Velopack"
            );
        }
        Err(err) => {
            warn!(error = %err, "Velopack update check failed");
        }
    }
}

fn try_check_and_stage_updates(source: &str) -> Result<(), velopack::Error> {
    let manager = UpdateManager::new(AutoSource::new(source), None, None)?;

    if let Some(update) = manager.get_update_pending_restart() {
        info!(
            version = %update.Version,
            package = %update.FileName,
            "Velopack update is already downloaded and will apply on the next restart"
        );
        return Ok(());
    }

    match manager.check_for_updates()? {
        UpdateCheck::UpdateAvailable(update) => {
            let version = update.TargetFullRelease.Version.clone();
            let package = update.TargetFullRelease.FileName.clone();
            info!(
                version = %version,
                package = %package,
                "Downloading Velopack update"
            );
            manager.download_updates(&update, None)?;
            info!(
                version = %version,
                package = %package,
                "Velopack update downloaded and will apply on the next restart"
            );
        }
        UpdateCheck::NoUpdateAvailable => {
            info!("No Velopack update is available");
        }
        UpdateCheck::RemoteIsEmpty => {
            info!("Velopack update feed is empty");
        }
    }

    Ok(())
}
