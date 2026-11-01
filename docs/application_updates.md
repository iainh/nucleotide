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

The vendor includes these post-release changes from upstream `develop`, audited through [4419cd4f](https://github.com/velopack/velopack/commit/4419cd4fe7eb5717b7dc24a79a67f494354831ae) on 2026-11-01:

- [Safe feed filenames](https://github.com/velopack/velopack/commit/de48f5349ea256e51eaff1b3b5d20d0d844b8d8e): store full, base and delta packages by their sanitized basename, and resolve the same basename when applying. Remote subpaths and URLs remain unchanged. Nucleotide also rejects empty and dot-only local names, including Windows trailing-space/dot aliases, because the upstream sanitizer alone retains `..`.
- [Delta download progress](https://github.com/velopack/velopack/commit/5852e3758a6bbad0f39aa4c247a4b05bf75509c3) and the [detached forwarder correction](https://github.com/velopack/velopack/commit/abb2622e56c691017e1259a9154b1d47a43d85dc): report progress during downloads without waiting indefinitely if a source retains a progress sender. Forwarded values can arrive after later direct progress values.
- [Actual `sq.version` write errors](https://github.com/velopack/velopack/commit/bbde968cf55b4145a29658b7e2f2387399bf8691): distinguish a missing manifest from a Windows filesystem or ZIP error.
- [Installed-version release notes](https://github.com/velopack/velopack/commit/f73e272ea2f4cb9954a6fc0d4ae138e3286aac22): expose markdown and HTML notes from the installed manifest through the locator and update manager. The getters trim packaging whitespace and return `None` for absent or whitespace-only notes.
- [Remove `derivative`](https://github.com/velopack/velopack/commit/c4e4af36b9da773e61b5927d34ca57021a721ee2): use handwritten `Default` implementations without changing manifest or package-name defaults. The unmaintained dependency and its advisory exception are no longer needed.

The [ZIP 9 upgrade](https://github.com/velopack/velopack/commit/ae2e8818973aa18ac63732a5c7ef9895116592f9) and fallible ZIP filename handling were already incorporated locally. The standalone vendor lockfile is synchronized to ZIP `9.2.0`, matching the workspace lockfile; it previously still selected ZIP 8 despite the updated manifest.

The standalone lockfile also includes upstream's [Rustls security update](https://github.com/velopack/velopack/commit/63e9be0c0e64cb7ef9c4b1753e42c10c40b8dbce) to `0.23.46` for RUSTSEC-2026-0346–0349. Its `rustls-webpki` (`0.103.16`), `webpki-root-certs` and `webpki-roots` (`1.0.11`) selections now match the workspace's existing patched versions. These lockfile updates do not change the explicit platform certificate verifier.

At the audit date, crates.io, NuGet's `vpk` feed and GitHub releases still list `1.2.161` as the latest stable release. The newer `1.3.39-g8d2844f` release is a prerelease, based on [8d2844f3](https://github.com/velopack/velopack/commit/8d2844f324a69bd848719ef465b1ae12c5060524). Keep the client and release CLI on the matching stable pins rather than moving the production release pipeline to a prerelease.

That prerelease includes Windows 7 support and a fix for delta creation in non-ASCII Windows paths; those CLI and updater-binary changes are not part of this Rust-client backport. Until a stable CLI includes the latter fix, use ASCII-only build and package paths on Windows. Upstream's later ICO fixes affect DIB frames without an AND mask; all five frames in Nucleotide's packaged icon are PNGs. Linux ARM packaging and changes to other language bindings and their dependencies are outside Nucleotide's managed-update platforms and Rust client.

The application and packaged smoke test still supply explicit `AutoSource` instances. They do not opt into the new default Velopack Flow source. No application adaptation to `HttpOptions` or the additive installed-version note getters is needed: the update UI continues to show notes for the available update from its feed asset, not notes for the currently installed version.

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
cargo test --manifest-path vendor/velopack-1.2.161/Cargo.toml --features public-utils --locked
```

Manager tests exercise subfolder, absolute URL, traversal and invalid names through download and apply APIs. They also read installed notes through the update manager, covering CDATA, HTML entities, absent notes, whitespace-only notes and independent markdown/HTML values. On Unix, a stand-in patch executable verifies base/delta path handling and a source retaining progress senders verifies download completion. Windows tests cover filename aliases and `sq.version` write failures; run them on Windows for runtime coverage. Two upstream download tests use public HTTPS endpoints.

For packaging commands, see `scripts/package-velopack.sh`, `scripts/package-velopack.ps1` and `docs/windows_install.md`.
