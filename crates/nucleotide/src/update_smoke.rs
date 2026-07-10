//! Packaged Velopack N -> N+1 smoke-test harness used only by release CI.

use std::{env, ffi::OsString, fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use velopack::{UpdateCheck, UpdateManager, VelopackApp, sources::AutoSource};

fn main() -> Result<()> {
    VelopackApp::build().run();

    let mut args = env::args_os().skip(1);
    let Some(command) = args.next() else {
        return Ok(());
    };
    match command.to_string_lossy().as_ref() {
        "apply" => apply(args),
        "verify" => verify(args),
        "reject-corrupt" => reject_corrupt(args),
        other => bail!("unknown update smoke-test command: {other}"),
    }
}

fn apply(mut args: impl Iterator<Item = OsString>) -> Result<()> {
    let source = required_argument(&mut args, "source")?;
    let result_path = required_argument(&mut args, "result path")?;
    let current_version = required_argument(&mut args, "current version")?;
    let target_version = required_argument(&mut args, "target version")?;

    let manager = manager(&source)?;
    ensure!(
        manager.get_current_version_as_string() == current_version.to_string_lossy(),
        "installed version did not match the expected source version"
    );
    let update = checked_update(&manager, &target_version)?;
    manager
        .download_updates(&update, None)
        .context("failed to download the smoke-test update")?;
    let pending = manager
        .get_update_pending_restart()
        .context("downloaded update was not staged for restart")?;
    ensure!(pending.Version == target_version.to_string_lossy());

    manager
        .wait_exit_then_apply_updates(
            pending,
            true,
            true,
            [
                OsString::from("verify"),
                source,
                result_path,
                target_version,
            ],
        )
        .context("failed to arm the smoke-test update")?;
    Ok(())
}

fn verify(mut args: impl Iterator<Item = OsString>) -> Result<()> {
    let source = required_argument(&mut args, "source")?;
    let result_path = required_argument(&mut args, "result path")?;
    let target_version = required_argument(&mut args, "target version")?;
    let manager = manager(&source)?;
    let installed = manager.get_current_version_as_string();
    ensure!(installed == target_version.to_string_lossy());
    fs::write(&result_path, installed).with_context(|| {
        format!(
            "failed to write update smoke-test result to {}",
            Path::new(&result_path).display()
        )
    })?;
    Ok(())
}

fn reject_corrupt(mut args: impl Iterator<Item = OsString>) -> Result<()> {
    let source = required_argument(&mut args, "source")?;
    let result_path = required_argument(&mut args, "result path")?;
    let target_version = required_argument(&mut args, "target version")?;
    let manager = manager(&source)?;
    let update = checked_update(&manager, &target_version)?;
    ensure!(
        manager.download_updates(&update, None).is_err(),
        "corrupt update unexpectedly passed verification"
    );
    fs::write(&result_path, "rejected").with_context(|| {
        format!(
            "failed to write corrupt-package result to {}",
            Path::new(&result_path).display()
        )
    })?;
    Ok(())
}

fn manager(source: &OsString) -> Result<UpdateManager> {
    let source = source
        .to_str()
        .context("smoke-test update source is not valid UTF-8")?;
    UpdateManager::new(AutoSource::new(source), None, None)
        .context("smoke-test executable is not running from a Velopack installation")
}

fn checked_update(
    manager: &UpdateManager,
    target_version: &OsString,
) -> Result<Box<velopack::UpdateInfo>> {
    let update = match manager
        .check_for_updates()
        .context("failed to check the local smoke-test feed")?
    {
        UpdateCheck::UpdateAvailable(update) => update,
        UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty => {
            bail!("local smoke-test feed did not contain an update")
        }
    };
    ensure!(update.TargetFullRelease.Version == target_version.to_string_lossy());
    Ok(update)
}

fn required_argument(args: &mut impl Iterator<Item = OsString>, name: &str) -> Result<OsString> {
    args.next()
        .with_context(|| format!("missing {name} argument"))
}
