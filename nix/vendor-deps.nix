# Vendor Rust dependencies for offline builds
{ lib
, stdenv
, fetchFromGitHub
, rustPlatform
, cargo
, git
}:

let
  # Fetch git dependencies manually
  gpui-src = fetchFromGitHub {
    owner = "zed-industries";
    repo = "zed";
    rev = "faa45c53d7754cfdd91d2f7edd3c786abc703ec7";
    sha256 = lib.fakeSha256; # Replace with actual hash after first run
  };
  
  helix-src = fetchFromGitHub {
    owner = "helix-editor";
    repo = "helix";
    rev = "a05c151bb6e8e9c65ec390b0ae2afe7a5efd619b"; # tag 25.07.1
    sha256 = lib.fakeSha256; # Replace with actual hash after first run
  };
in

stdenv.mkDerivation {
  name = "nucleotide-vendor";
  
  src = ../.;
  
  nativeBuildInputs = [ cargo git ];
  
  buildPhase = ''
    # Create vendor directory
    mkdir -p vendor
    
    # Set up cargo config for vendoring
    export CARGO_HOME=$(pwd)/.cargo
    mkdir -p $CARGO_HOME
    
    # Vendor all dependencies
    cargo vendor vendor > config.toml
    
    # Patch Cargo.toml to use vendored sources
    cat >> .cargo/config.toml <<EOF
    
    [source.crates-io]
    replace-with = "vendored-sources"
    
    [source.vendored-sources]
    directory = "vendor"
    
    [source."https://github.com/zed-industries/zed"]
    git = "https://github.com/zed-industries/zed"
    rev = "faa45c53d7754cfdd91d2f7edd3c786abc703ec7"
    replace-with = "vendored-zed"
    
    [source.vendored-zed]
    directory = "${gpui-src}"
    
    [source."https://github.com/helix-editor/helix"]
    git = "https://github.com/helix-editor/helix"
    tag = "25.07.1"
    replace-with = "vendored-helix"
    
    [source.vendored-helix]
    directory = "${helix-src}"
    EOF
  '';
  
  installPhase = ''
    mkdir -p $out
    cp -r vendor $out/
    cp -r .cargo $out/
  '';
}