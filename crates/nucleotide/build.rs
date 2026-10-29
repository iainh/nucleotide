fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&manifest_dir)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };

    // Resolve paths through Git so linked worktrees and packed refs also rebuild
    // the embedded revision after a checkout or commit.
    for path in ["HEAD", "refs", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=NUCLEOTIDE_GIT_COMMIT={commit}");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "windows" {
        let rc_file = std::path::Path::new("resources/windows/nucleotide.rc");

        println!("cargo:rerun-if-changed={}", rc_file.display());

        embed_resource::compile(rc_file, embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
