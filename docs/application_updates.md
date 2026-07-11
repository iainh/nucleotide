# Application updates

Nucleotide uses Velopack to build, discover, download and apply application updates on macOS and Windows. The update controller is process-wide, so every window shows the same state and download progress.

Linux archives remain manually installed. Source and portable builds report that managed updates are unavailable.

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

## Build a release

The `release.yml` workflow runs for a version tag such as `v0.5.1`. The tag must match `workspace.package.version` in `Cargo.toml`.

The workflow performs these steps:

1. Build the Linux archive and remote helpers.
2. Build the macOS universal application and Windows package input.
3. Download each platform's previous published Velopack release so `vpk pack` can generate delta packages.
4. Generate release notes from commits since the previous tag.
5. Sign and notarize the macOS application and installer when Apple credentials are configured, and sign Windows binaries and installers when Windows signing secrets are configured. Otherwise, produce unsigned packages for that platform.
6. Upload both Velopack channels into one draft GitHub release.
7. Upload the Linux files, checksums and build-provenance attestations.
8. Verify the feeds, full packages and installers, then publish the release.

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

For packaging commands, see `scripts/package-velopack.sh`, `scripts/package-velopack.ps1` and `docs/windows_install.md`.
