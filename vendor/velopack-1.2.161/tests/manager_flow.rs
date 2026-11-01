use std::{fs, path::PathBuf};

use velopack::{locator::VelopackLocatorConfig, UpdateManager};

fn test_manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("Test.Squirrel-App.nuspec")
}

#[test]
fn new_flow_creates_manager_without_explicit_source() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root_dir = temp_dir.path().join("root");
    let packages_dir = root_dir.join("packages");
    let current_binary_dir = root_dir.join("current");
    let update_exe_path = root_dir.join("Update.exe");

    fs::create_dir_all(&packages_dir).unwrap();
    fs::create_dir_all(&current_binary_dir).unwrap();
    fs::write(&update_exe_path, []).unwrap();

    let mut locator = VelopackLocatorConfig {
        RootAppDir: root_dir,
        UpdateExePath: update_exe_path,
        PackagesDir: packages_dir,
        ManifestPath: test_manifest_path(),
        CurrentBinaryDir: current_binary_dir,
        IsPortable: true,
    };

    let manager = UpdateManager::new_flow(None, Some(locator.clone())).unwrap();

    assert_eq!(manager.get_app_id(), "Test.Squirrel-App");
    assert_eq!(manager.get_current_version_as_string(), "1.0.0");
    assert_eq!(manager.get_current_release_notes_markdown(), None);
    assert_eq!(manager.get_current_release_notes_html(), None);

    let manifest = fs::read_to_string(test_manifest_path()).unwrap();
    locator.ManifestPath = locator.CurrentBinaryDir.join("sq.version");
    for (markdown, html, expected_markdown, expected_html) in [
        (
            "\n# Release 1.0.0\n\n- Fixed <everything>\n",
            "\n<h1>Notes</h1>\n<p>Fixed &lt;everything&gt;</p>\n",
            Some("# Release 1.0.0\n\n- Fixed <everything>"),
            Some("<h1>Notes</h1>\n<p>Fixed &lt;everything&gt;</p>"),
        ),
        (" \n\t ", "", None, None),
        ("# Markdown only", " \n ", Some("# Markdown only"), None),
        ("", "<p>HTML only</p>", None, Some("<p>HTML only</p>")),
    ] {
        let notes = format!(
            "<releaseNotes><![CDATA[{markdown}]]></releaseNotes>\n\
             <releaseNotesHtml><![CDATA[{html}]]></releaseNotesHtml>\n</metadata>"
        );
        fs::write(&locator.ManifestPath, manifest.replace("</metadata>", &notes)).unwrap();
        let manager = UpdateManager::new_flow(None, Some(locator.clone())).unwrap();
        assert_eq!(manager.get_current_release_notes_markdown().as_deref(), expected_markdown);
        assert_eq!(manager.get_current_release_notes_html().as_deref(), expected_html);
    }
}
