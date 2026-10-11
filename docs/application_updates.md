# Application updates

Nucleotide uses Velopack to build, discover, download and apply application updates on macOS and Windows. The update controller is process-wide, so every window shows the same state and download progress.

Linux archives remain manually installed, and unpackaged development builds report that managed updates are unavailable. Windows Velopack portable bundles support managed updates after extraction.

## User experience

Nucleotide checks for updates after a short startup delay and every six hours while it remains open. It also checks when a window becomes active after the previous result becomes stale.

The titlebar stays unchanged when Nucleotide is up to date. An indicator appears for these actionable states:

- An update is available.
- An update is downloading in the background.
- A downloaded update is ready for restart.
- A manual check or update operation is in progress.
- An operation failed and can be retried.

Select the indicator to view the version, download size, release notes and available action. Nucleotide never downloads an update automatically unless `auto_download` is enabled.

Before applying an update, Nucleotide checks for modified buffers. It asks before saving them, waits for writes to finish and only then arms Velopack and exits. If a buffer cannot be saved, the application stays open.

## Configure update checks

Add the following section to `nucleotide.toml`:

```toml
[updates]
enabled = true
check_on_startup = true
auto_download = false
```

Set `NUCLEOTIDE_DISABLE_AUTO_UPDATE=1` to disable update support for a process. Developers can set `NUCLEOTIDE_UPDATE_SOURCE` to test against another Velopack source. The application does not expose this source override in user configuration.

## Vendored updater

The Rust client and release CLI are pinned to Velopack `1.2.161`, published on 2026-09-29. The vendor directory comes from the [released crate](https://static.crates.io/crates/velopack/velopack-1.2.161.crate), with SHA-256 `4295172ffaad78f9adbab35f7ffefa97e4ceefee9d2c38bda37fdce4d6c66e5a`, verified against the crates.io index.

Nucleotide retains `ureq`'s `platform-verifier` feature and explicitly selects `RootCerts::PlatformVerifier` for feed and package requests. Upstream's release does not select this configuration. This preserves system certificate trust, including enterprise roots; a configuration assertion or successful public HTTPS request does not prove that a particular enterprise certificate works on macOS or Windows.

The vendor includes these post-release fixes from upstream main audited at [301077f4](https://github.com/velopack/velopack/commit/301077f4792250d5ff4eff1ff4f22021421a506b):

- [Safe feed filenames](https://github.com/velopack/velopack/commit/de48f5349ea256e51eaff1b3b5d20d0d844b8d8e): store full, base and delta packages by their sanitized basename, and resolve the same basename when applying. Remote subpaths and URLs remain unchanged. Nucleotide also rejects empty and dot-only local names, including Windows trailing-space/dot aliases, because the upstream sanitizer alone retains `..`.
- [Delta download progress](https://github.com/velopack/velopack/commit/5852e3758a6bbad0f39aa4c247a4b05bf75509c3) and the [detached forwarder correction](https://github.com/velopack/velopack/commit/abb2622e56c691017e1259a9154b1d47a43d85dc): report progress during downloads without waiting indefinitely if a source retains a progress sender. Forwarded values can arrive after later direct progress values.
- [Actual `sq.version` write errors](https://github.com/velopack/velopack/commit/bbde968cf55b4145a29658b7e2f2387399bf8691): distinguish a missing manifest from a Windows filesystem or ZIP error.

The application and packaged smoke test still supply explicit `AutoSource` instances. They do not opt into the new default Velopack Flow source. No application adaptation to `HttpOptions` is needed.

## Build a release

The `release.yml` workflow runs for a version tag such as `v0.5.1`. The tag must match `workspace.package.version` in `Cargo.toml`.

The workflow performs these steps:

1. Build the Linux archive and remote helpers.
2. Build the macOS universal application and Windows package input.
3. Download each platform's previous published Velopack release so `vpk pack` can generate delta packages.
4. Generate release notes from commits since the previous tag.
5. Sign and notarize the macOS application and installer when Apple credentials are configured, and sign the Windows binaries included in portable bundles when Windows signing secrets are configured. Otherwise, produce unsigned packages for that platform.
6. Upload both Velopack channels into one draft GitHub release.
7. Upload the Linux files, checksums and build-provenance attestations.
8. Verify the feeds, full packages, macOS installer and Windows portable bundle, then publish the release.

The draft is the promotion boundary. Velopack clients cannot discover the release until every required asset passes verification and the workflow publishes it.

## Configure release secrets

Create a protected GitHub environment named `release`. Configure approval rules as needed and add these repository or environment secrets:

| Secret | Purpose |
| --- | --- |
| `MACOS_BUILD_CERTIFICATE_BASE64` | Optional base64-encoded Developer ID Application `.p12` certificate |
| `MACOS_INSTALLER_CERTIFICATE_BASE64` | Optional base64-encoded Developer ID Installer `.p12` certificate |
| `MACOS_CERTIFICATE_PASSWORD` | Optional password for both macOS certificate files |
| `MACOS_SIGN_APP_IDENTITY` | Optional Developer ID Application identity without the team suffix |
| `MACOS_SIGN_INSTALL_IDENTITY` | Optional Developer ID Installer identity without the team suffix |
| `APPLE_ID` | Optional Apple account used for notarization |
| `APPLE_APP_PASSWORD` | Optional app-specific password used by `notarytool` |
| `APPLE_TEAM_ID` | Optional Apple Developer team identifier |
| `WINDOWS_SIGNING_CERTIFICATE_BASE64` | Optional base64-encoded Authenticode `.pfx` certificate |
| `WINDOWS_SIGNING_CERTIFICATE_PASSWORD` | Optional password for the Windows certificate |

Configure all eight Apple signing and notarization secrets to produce signed and notarized macOS packages, or omit all eight to produce unsigned packages. Configure both Windows signing secrets to produce signed packages, or omit both to produce unsigned packages. A partial configuration for either platform fails the build. When configured, the workflow imports certificates into temporary stores and removes them in an `always()` cleanup step. It pins the Velopack CLI to the same version as the Rust dependency.

## Recover from a bad release

Do not replace an already published package in place. Fix the problem, increment the application version and publish a new release. Clients will discover the newer version through the platform feed.

If publication fails, leave the GitHub release as a draft while correcting the pipeline. A draft feed is not visible to installed clients.

## Test locally

Unit tests cover state visibility, metadata conversion, restart argument sanitization and the serialized worker sequence. Release CI also installs synthetic package N, updates it to N+1 from a local feed, verifies the restarted version and confirms that a corrupt N+2 package is rejected. This packaged smoke test runs on macOS and Windows before publication.

Run the focused tests with:

```bash
cargo test -p nucleotide updates::
```

Run the vendored client's unit, source, manager and documentation tests separately. Its upstream integration tests require `public-utils`:

```bash
cargo test --manifest-path vendor/velopack-1.2.161/Cargo.toml --features public-utils
```

Manager tests exercise subfolder, absolute URL, traversal and invalid names through download and apply APIs. On Unix, a stand-in patch executable verifies base/delta path handling and a source retaining progress senders verifies download completion. Windows tests cover filename aliases and `sq.version` write failures; run them on Windows for runtime coverage. Two upstream download tests use public HTTPS endpoints.

For packaging commands, see `scripts/package-velopack.sh`, `scripts/package-velopack.ps1` and `docs/windows_install.md`.
