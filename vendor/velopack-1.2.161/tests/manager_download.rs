mod common;

use common::*;
use sha2::Digest;
use std::{fs, path::PathBuf, sync::mpsc};
use velopack::{
    locator::VelopackLocatorConfig,
    sources::{HttpSource, NoneSource},
    Error, UpdateInfo, UpdateManager, VelopackAsset,
};

fn test_manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("Test.Squirrel-App.nuspec")
}

/// Creates an installed app layout under `temp_dir` (`root/packages` is the packages dir).
fn test_locator(temp_dir: &std::path::Path) -> VelopackLocatorConfig {
    let root_dir = temp_dir.join("root");
    let packages_dir = root_dir.join("packages");
    let current_binary_dir = root_dir.join("current");
    let update_exe_path = root_dir.join("Update.exe");

    fs::create_dir_all(&packages_dir).unwrap();
    fs::create_dir_all(&current_binary_dir).unwrap();
    fs::write(&update_exe_path, []).unwrap();

    VelopackLocatorConfig {
        RootAppDir: root_dir,
        UpdateExePath: update_exe_path,
        PackagesDir: packages_dir,
        ManifestPath: test_manifest_path(),
        CurrentBinaryDir: current_binary_dir,
        IsPortable: true,
    }
}

/// Downloads a full update whose feed FileName is `file_name` from a mock server into a fresh app layout,
/// returning the temp dir holding that layout and the package bytes that were served.
fn download_full_update(file_name: &str) -> (tempfile::TempDir, Vec<u8>) {
    let temp_dir = tempfile::tempdir().unwrap();
    let locator = test_locator(temp_dir.path());

    let body: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    let server = MockHttpServer::empty();
    server.add_route(MockRoute {
        path_contains: "Test.Squirrel-App-2.0.0-full.nupkg".to_string(),
        response_code: 200,
        response_body: body.clone(),
        expected_headers: vec![],
    });

    let update = UpdateInfo {
        TargetFullRelease: VelopackAsset {
            PackageId: "Test.Squirrel-App".to_string(),
            Version: "2.0.0".to_string(),
            Type: "Full".to_string(),
            FileName: file_name.to_string(),
            SHA256: sha2::Sha256::digest(&body).iter().map(|b| format!("{:02X}", b)).collect(),
            Size: body.len() as u64,
            ..Default::default()
        },
        ..Default::default()
    };

    let manager = UpdateManager::new(HttpSource::new(server.url()), None, Some(locator)).unwrap();
    manager.download_updates(&update, None).unwrap();
    (temp_dir, body)
}

#[test]
fn download_updates_stores_subfolder_asset_in_packages_dir() {
    // the feed may point at packages in a sub-folder next to releases.{channel}.json (velopack/velopack#596)
    let (temp_dir, body) = download_full_update("2.0.0/Test.Squirrel-App-2.0.0-full.nupkg");
    let packages_dir = temp_dir.path().join("root").join("packages");
    assert_eq!(fs::read(packages_dir.join("Test.Squirrel-App-2.0.0-full.nupkg")).unwrap(), body);
    assert!(!packages_dir.join("2.0.0").exists());
}

#[test]
fn download_updates_keeps_traversing_asset_names_inside_packages_dir() {
    let (temp_dir, body) = download_full_update("../../Test.Squirrel-App-2.0.0-full.nupkg");
    let packages_dir = temp_dir.path().join("root").join("packages");
    assert_eq!(fs::read(packages_dir.join("Test.Squirrel-App-2.0.0-full.nupkg")).unwrap(), body);
    // packages/../../ is the temp dir itself, nothing may be written there
    assert!(!temp_dir.path().join("Test.Squirrel-App-2.0.0-full.nupkg").exists());
}

#[test]
fn download_updates_stores_absolute_url_asset_in_packages_dir() {
    let server = MockHttpServer::empty();
    let body = vec![7, 3, 9, 1];
    server.add_route(MockRoute {
        path_contains: "GET /cdn/App-full.nupkg ".into(),
        response_code: 200,
        response_body: body.clone(),
        expected_headers: vec![],
    });
    let temp_dir = tempfile::tempdir().unwrap();
    let locator = test_locator(temp_dir.path());
    let package = locator.PackagesDir.join("App-full.nupkg");
    let update = UpdateInfo {
        TargetFullRelease: VelopackAsset {
            FileName: format!("{}/cdn/App-full.nupkg", server.url()),
            Size: body.len() as u64,
            SHA256: sha2::Sha256::digest(&body).iter().map(|b| format!("{:02X}", b)).collect(),
            ..Default::default()
        },
        ..Default::default()
    };
    let manager = UpdateManager::new(HttpSource::new("http://feed.invalid"), None, Some(locator)).unwrap();
    manager.download_updates(&update, None).unwrap();
    assert_eq!(fs::read(package).unwrap(), body);
}

#[test]
fn download_and_apply_reject_non_file_asset_names() {
    let mut names = vec!["", ".", "..", "../..", "packages/"];
    if cfg!(windows) {
        names.extend([".. ", "...", " ", "..\\.."]);
    }
    for name in names {
        let temp_dir = tempfile::tempdir().unwrap();
        let locator = test_locator(temp_dir.path());
        let packages = locator.PackagesDir.clone();
        let manager = UpdateManager::new(NoneSource {}, None, Some(locator)).unwrap();
        let update = UpdateInfo {
            TargetFullRelease: VelopackAsset {
                FileName: name.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            matches!(manager.download_updates(&update, None), Err(Error::InvalidPackage(_))),
            "{name:?}"
        );
        assert!(
            matches!(
                manager.wait_exit_then_apply_updates(&update.TargetFullRelease, false, false, Vec::<String>::new()),
                Err(Error::InvalidPackage(_))
            ),
            "{name:?}"
        );
        assert!(!packages.join(".partial").exists());
        assert!(!packages.join("..partial").exists());
    }
}

#[test]
fn apply_resolves_feed_names_to_local_package_files() {
    let temp_dir = tempfile::tempdir().unwrap();
    let locator = test_locator(temp_dir.path());
    let expected = locator.PackagesDir.join("App-full.nupkg");
    let manager = UpdateManager::new(NoneSource {}, None, Some(locator)).unwrap();
    for name in [
        "2.0/App-full.nupkg",
        "../../App-full.nupkg",
        "/outside/App-full.nupkg",
        "https://cdn.invalid/App-full.nupkg",
    ] {
        let asset = VelopackAsset {
            FileName: name.into(),
            ..Default::default()
        };
        match manager.wait_exit_then_apply_updates(&asset, false, false, Vec::<String>::new()) {
            Err(Error::FileNotFound(path)) => assert_eq!(path, expected),
            other => panic!("Expected missing local package, got {other:?}"),
        }
    }
}

#[test]
fn download_updates_reports_progress_while_downloading_deltas() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root_dir = temp_dir.path().join("root");
    let packages_dir = root_dir.join("packages");
    let current_binary_dir = root_dir.join("current");
    let update_exe_path = root_dir.join("Update.exe");

    fs::create_dir_all(&packages_dir).unwrap();
    fs::create_dir_all(&current_binary_dir).unwrap();
    fs::write(&update_exe_path, []).unwrap();

    let locator = VelopackLocatorConfig {
        RootAppDir: root_dir,
        UpdateExePath: update_exe_path,
        PackagesDir: packages_dir,
        ManifestPath: test_manifest_path(),
        CurrentBinaryDir: current_binary_dir,
        IsPortable: true,
    };

    let size = 1024 * 1024;
    let server = MockHttpServer::empty();
    server.add_route(MockRoute {
        path_contains: "-delta.nupkg".to_string(),
        response_code: 200,
        response_body: vec![0u8; size],
        expected_headers: vec![],
    });

    let asset = |file_name: &str, version: &str, asset_type: &str| VelopackAsset {
        PackageId: "Test.Squirrel-App".to_string(),
        Version: version.to_string(),
        Type: asset_type.to_string(),
        FileName: file_name.to_string(),
        Size: size as u64,
        ..Default::default()
    };
    let update = UpdateInfo {
        TargetFullRelease: asset("Test.Squirrel-App-2.0.0-full.nupkg", "2.0.0", "Full"),
        BaseRelease: Some(asset("Test.Squirrel-App-1.0.0-full.nupkg", "1.0.0", "Full")),
        DeltasToTarget: vec![asset("Test.Squirrel-App-2.0.0-delta.nupkg", "2.0.0", "Delta")],
        IsDowngrade: false,
    };

    let manager = UpdateManager::new(HttpSource::new(server.url()), None, Some(locator)).unwrap();
    let (sender, receiver) = mpsc::channel();
    // the delta fails its checksum once downloaded and the full package isn't served, so this
    // errors, and only the progress reported along the way matters here
    let _ = manager.download_updates(&update, Some(sender));

    // ends once every sender is dropped, including the forwarding thread's, which may outlive the call
    let progress: Vec<i16> = receiver.iter().collect();
    assert!(
        progress.iter().any(|p| *p > 0 && *p < 70),
        "no progress while downloading the delta: {:?}",
        progress
    );
}

#[cfg(unix)]
#[test]
fn delta_updates_use_local_filenames_and_do_not_join_retained_progress_senders() {
    use std::{
        os::unix::fs::PermissionsExt,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use velopack::{bundle::Manifest, sources::UpdateSource, VelopackAssetFeed};

    #[derive(Clone)]
    struct DeltaSource(Arc<Mutex<Vec<mpsc::Sender<i16>>>>);
    impl UpdateSource for DeltaSource {
        fn get_release_feed(&self, _: &str, _: &Manifest, _: &str) -> Result<VelopackAssetFeed, Error> {
            unreachable!()
        }
        fn download_release_entry(&self, asset: &VelopackAsset, dest: &std::path::Path, progress: Option<mpsc::Sender<i16>>) -> Result<(), Error> {
            if asset.Type != "Delta" {
                return Err(Error::NotSupported("Full fallback must not be needed".into()));
            }
            fs::write(dest, b"delta bytes")?;
            if let Some(sender) = progress {
                sender.send(if asset.Version == "1.5.0" { 35 } else { 50 }).unwrap();
                self.0.lock().unwrap().push(sender);
            }
            Ok(())
        }
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let locator = test_locator(temp_dir.path());
    let packages = locator.PackagesDir.clone();
    fs::write(packages.join("App-1.0-full.nupkg"), b"patched full bytes").unwrap();
    // Stand-in for the native patch process: require both delta files and copy the old package.
    fs::write(
        &locator.UpdateExePath,
        "#!/bin/sh\nset -eu\n[ -f \"$7\" ]\n[ -f \"$9\" ]\ncp \"$3\" \"$5\"\n",
    )
    .unwrap();
    fs::set_permissions(&locator.UpdateExePath, fs::Permissions::from_mode(0o755)).unwrap();
    let delta = |name: &str, version: &str| VelopackAsset {
        FileName: name.into(),
        Version: version.into(),
        Type: "Delta".into(),
        Size: 11,
        SHA256: sha2::Sha256::digest(b"delta bytes").iter().map(|b| format!("{:02X}", b)).collect(),
        ..Default::default()
    };
    let update = UpdateInfo {
        TargetFullRelease: VelopackAsset {
            FileName: "2.0/App-2.0-full.nupkg".into(),
            ..Default::default()
        },
        BaseRelease: Some(VelopackAsset {
            FileName: "../../App-1.0-full.nupkg".into(),
            ..Default::default()
        }),
        DeltasToTarget: vec![
            delta("../../App-1.5-delta.nupkg", "1.5.0"),
            delta("https://cdn.invalid/App-2.0-delta.nupkg", "2.0.0"),
        ],
        ..Default::default()
    };
    let source = DeltaSource(Arc::new(Mutex::new(Vec::new())));
    let manager = UpdateManager::new(source.clone(), None, Some(locator)).unwrap();
    let (sender, progress) = mpsc::channel();
    let (done_sender, done) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        done_sender.send(manager.download_updates(&update, Some(sender))).unwrap();
    });
    let result = done.recv_timeout(Duration::from_secs(5));
    // Always release the retained senders, even if a joined forwarder blocked the worker.
    source.0.lock().unwrap().clear();
    result.expect("Update must finish while the source still holds progress senders").unwrap();
    worker.join().unwrap();
    let values: Vec<_> = progress.iter().collect();
    assert!(
        values.contains(&12) && values.contains(&52),
        "Expected progress within both delta ranges: {values:?}"
    );
    assert_eq!(fs::read(packages.join("App-2.0-full.nupkg")).unwrap(), b"patched full bytes");
    assert!(!temp_dir.path().join("App-1.5-delta.nupkg").exists());
}

#[cfg(windows)]
#[test]
fn extraction_distinguishes_missing_manifest_from_sq_version_write_errors() {
    use std::io::{Cursor, Write};
    use velopack::bundle::load_bundle_from_memory;

    for has_manifest in [false, true] {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        if has_manifest {
            zip.start_file("App.nuspec", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(b"manifest bytes").unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        let bundle = load_bundle_from_memory(&bytes).unwrap();
        let current = tempfile::tempdir().unwrap();
        // A directory at the target path deterministically causes a write error, without relying on ACLs.
        fs::create_dir(current.path().join("sq.version")).unwrap();
        let error = bundle.extract_lib_contents_to_path(current.path(), |_| {}).unwrap_err();
        if has_manifest {
            assert!(matches!(error, Error::Io(_)), "{error:?}");
        } else {
            assert!(
                matches!(error, Error::InvalidPackage(ref message) if message == "No .nuspec manifest found"),
                "{error:?}"
            );
        }
    }
}
