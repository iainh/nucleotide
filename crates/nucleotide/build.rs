fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "windows" {
        let rc_file = std::path::Path::new("resources/windows/nucleotide.rc");
        let icon_file = std::path::Path::new("assets/nucleotide.ico");

        println!("cargo:rerun-if-changed={}", rc_file.display());
        println!("cargo:rerun-if-changed={}", icon_file.display());

        embed_resource::compile(rc_file, embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
