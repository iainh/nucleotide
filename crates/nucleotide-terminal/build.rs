use std::path::{Path, PathBuf};

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let Some(include_dir) = std::env::var_os("DEP_GHOSTTY_VT_INCLUDE").map(PathBuf::from) else {
        return;
    };
    let Some(install_dir) = include_dir.parent() else {
        return;
    };
    let source = install_dir.join("bin").join("ghostty-vt.dll");

    println!("cargo:rerun-if-changed={}", source.display());

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR must be set"));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR must be inside a Cargo profile directory");

    copy_if_changed(&source, &profile_dir.join("ghostty-vt.dll"));
    copy_if_changed(&source, &profile_dir.join("deps").join("ghostty-vt.dll"));
}

fn copy_if_changed(source: &Path, destination: &Path) {
    let source_bytes = std::fs::read(source)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source.display()));
    if std::fs::read(destination).is_ok_and(|destination_bytes| destination_bytes == source_bytes) {
        return;
    }

    std::fs::copy(source, destination).unwrap_or_else(|error| {
        panic!(
            "failed to copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });
}
