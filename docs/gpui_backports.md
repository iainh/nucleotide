# GPUI backports

Nucleotide keeps its vendored Zed base at
[`51fac82ddd`](https://github.com/zed-industries/zed/commit/51fac82ddd60380b4ea56d9e02b895628742c454)
(22 June 2026). These focused backports avoid the `gpui_apple` migration and
Rust 1.98.1 requirement of the audited
[`93d7456688`](https://github.com/zed-industries/zed/commit/93d745668805029476e6d7fbb19652116fed51de).
The application still builds with Rust 1.96.0.

The 1 November 2026 follow-up audit compares that previous audit point with
upstream `main` at
[`d94f32a1d9`](https://github.com/zed-industries/zed/commit/d94f32a1d948c273ab7d5a9fd344e3fd22f971eb).
It retains the same base and adds the focused corrections below. No application
API migrations, root manifest changes, or lockfile changes are required.

## Backported corrections

| Upstream commit | Correction and adaptation |
| --- | --- |
| [`89e8a4b9ec`](https://github.com/zed-industries/zed/commit/89e8a4b9ec7e0fa8a49987a3c1dbf29343dc6999) | DirectWrite nullable arrays, mutable callback-context provenance, and bounded DirectX atlas uploads. The existing staging-texture `Unmap` was already correct and stays unchanged. |
| [`ac6818fb2d`](https://github.com/zed-industries/zed/commit/ac6818fb2de7aaec9ee00d4ff697e948c8b8f0a3) | Enable DirectWrite's default `liga`, `clig`, and `calt` features even when no features are configured. Explicit disable settings still win. |
| [`b9419ae728`](https://github.com/zed-industries/zed/commit/b9419ae72814157164b09592ab2466f9f4709f09) | Correct the reversed linear/sRGB HLSL transfer exponents to match Metal and WGSL. |
| [`1d7e5f1d01`](https://github.com/zed-industries/zed/commit/1d7e5f1d014acf0d288c6d11be93c31fe647e725) | Convert initial window bounds with the target monitor's DPI, not the temporary window's DPI. |
| [`2c4e44704c`](https://github.com/zed-industries/zed/commit/2c4e44704c37ee87e59ac84e3e17388178b28545), [`72656afa6d`](https://github.com/zed-industries/zed/commit/72656afa6d477d57ca875b1b7c8423483e32dc74) | Leave scheduled tasks pending when timer/thread-pool submission fails during shutdown. Windows uses the follow-up Win32 callback API, retaining priority and timer-resolution behaviour. |
| [`96e95edac4`](https://github.com/zed-industries/zed/commit/96e95edac45b0e0696179b8139aa521cf48db392) | Return optional texture handles and skip stale sprite batches on WGPU, Metal, and DirectX. Adapted to this base's atlas state and renderer APIs, without introducing the newer shared atlas backend or bind-group cache. A skipped batch is successful, not an instance-buffer overflow that should trigger a retry. |
| [`dd68454633`](https://github.com/zed-industries/zed/commit/dd68454633ac9fea8b42115ae09568e7395831e2) | Raise the Wayland backend floor to 0.3.15 for inert-proxy destruction safety. The root lockfile already resolves 0.3.17; the vendor lockfile receives the upstream's three-package update. |
| [`5079b33d65`](https://github.com/zed-industries/zed/commit/5079b33d657593ffd8a2d2978534988a8b40867e) | Cache committed IME cursor rectangles in protocol coordinates, avoiding the Fcitx5/KWin commit feedback loop. Preserve Nucleotide's existing composition guard and only advance the cache when a rectangle is committed. |
| [`96ce8f2a05`](https://github.com/zed-industries/zed/commit/96ce8f2a05f8912851e5d20d808fe21f4134bd45) | Use one immortal CVDisplayLink per display and a per-window dispatch source. This removes the callback context's dependency on a released per-window source. Adapt window registration to the existing screen helper and guard a temporarily absent screen. |
| [`382ff7a64b`](https://github.com/zed-industries/zed/commit/382ff7a64b334ab59a3dc0e3f3185705f79cd0ab) | Stop synthetic drag replay when its original button is released or the window disappears. Keep the 16 ms interval and generation cancellation. |
| [`f25434f3c5`](https://github.com/zed-industries/zed/commit/f25434f3c5a895d52f402ab05c079aa6703825c1) | Consume X11 GPU-recovery repaint demand inside `refresh`, so every caller observes it. This base already coalesces Expose events through the refresh timer; preserve that presentation behaviour rather than importing upstream's newer event loop. |

### November follow-up

| Upstream commit | Correction and adaptation |
| --- | --- |
| [`2529740084`](https://github.com/zed-industries/zed/commit/2529740084edf8c736e9d316edbcf56d40c1cf0f) | Allocate the element arena behind `Rc`, so arena access through the draw thread-local does not alias an inline field covered by `&mut App`. Retain this base's arena-clear API. |
| [`e4a3732b94`](https://github.com/zed-industries/zed/commit/e4a3732b94eb0881e1d954201ed41ffd2abf18f6) | Pool only character-width caches, not wrappers owning the text system. Preserve the local font-run pool and wrapping API. |
| [`370591f3d1`](https://github.com/zed-industries/zed/commit/370591f3d178718e3a822cfbf2a5879cdcccfad1) | Defer native-close cleanup to the foreground executor, including system-tab removal. App-driven closure may already have removed the window. |
| [`bc7041c600`](https://github.com/zed-industries/zed/commit/bc7041c600e3da82713849f4d65aa4a3cac89f67) | Include standalone modifiers in the existing one-second pending-keystroke timeout. Keep this base's timer task rather than importing the newer pausable timeout implementation. |
| [`99e70ee4a0`](https://github.com/zed-industries/zed/commit/99e70ee4a02d8dcf1b72f487f81aaf0d3be74f76) | Measure animations with the executor clock. This base has no spring-animation element to update. |
| [`8fa085e397`](https://github.com/zed-industries/zed/commit/8fa085e397fd106166eb055f74a6e9d5e674d02f) | Include inherited opacity in cached-view reuse and propagate opacity during prepaint. Record actual painted opacity and request another frame when hover or drag-over changes it. Adapt to `AnyView::cached`, without the newer hot-reload view implementation. |
| [`dc2a339d5d`](https://github.com/zed-industries/zed/commit/dc2a339d5d043da448a3f7ddc7c0a85c63864aad), [`4d847859f5`](https://github.com/zed-industries/zed/commit/4d847859f5506471bd80ca78ee6ced161b19a4e8) | Track key/pointer presses in arrival order, including serial zero after wraparound; ignore release and unrelated protocol serials for selection ownership. Pair activation serials with their original live focused surface and store each pending operation on its token, avoiding races between overlapping requests. Retain raw serial APIs for unrelated callers instead of importing upstream's serial newtypes. |
| [`e462f873c6`](https://github.com/zed-industries/zed/commit/e462f873c622f6265736bf8468e7a017f4ba99de) | Validate private Windows messages before interpreting their payloads. Dispatch keyboard accelerators directly to known GPUI windows. Cursor, GPU-loss, and forced-update senders all supply the validation number; retain local forced GPU-recovery rendering. |
| [`979cd3fdf0`](https://github.com/zed-industries/zed/commit/979cd3fdf02ffa39131b33507cbd7375a68910fd) | Shape Unicode line/paragraph-separated segments on one baseline, preserving byte indices and font-run boundaries. Keep custom DirectWrite settings, font caches, and GPU rendering. |
| [`e18de65b75`](https://github.com/zed-industries/zed/commit/e18de65b75901f8720ff9c9320bb013395de54e5) | Use source-over destination alpha for Metal/DirectX layers and WGPU paths. The Metal correction is adapted from `gpui_apple` to the retained `gpui_macos` renderer. |
| [`9f305aaf88`](https://github.com/zed-industries/zed/commit/9f305aaf88370e8ca39c702242186e3d8f97ecca) | Return an error when AppKit cannot create an open/save panel. Check raw Objective-C pointers without requiring the objc2 binding migration. |
| [`4112666801`](https://github.com/zed-industries/zed/commit/411266680135a2e89c757d8c677dca20defe8cbb) | Correct Spanish-ISO shortcut mappings without changing other keyboard layouts. |

## Retained local behaviour

- Custom DirectWrite rendering settings, system font-smoothing monitoring,
  negative subpixel glyph origins, deferred DirectX uploads, and renderer
  optimizations remain in place.
- The 9 October drag-offset fix still uses `mouse_down.position`, not the
  move event that crosses the drag threshold.
- macOS font-family deduplication is already source-equivalent to
  [`20a93f6195`](https://github.com/zed-industries/zed/commit/20a93f6195ca8e9f0a748038317a5efe1be3e482).
  Keep the separate Core Text Create Rule ownership correction too.
- The macOS blur material already uses `UnderWindowBackground`, as in
  [`90d8514059`](https://github.com/zed-industries/zed/commit/90d8514059561f0e44af0a32e84e1c329cef3232).
  Its existing default blending mode is `BehindWindow`; no visual restyling
  is needed.
- The shader build script already resolves sources from `CARGO_MANIFEST_DIR`.
- Application caption handlers remain necessary. The reverted native-titlebar
  experiment is not reinstated without Windows runtime testing.
- `InteractiveElement::capture_drop` adds typed capture-phase drops using bounds
  containment, so occluding descendants such as resize handles cannot block a
  window-wide external file drop. Existing `on_drop` handlers retain their
  bubble-phase hover checks and `can_drop` predicates.

## Deferred migration

[`eb354c8d50`](https://github.com/zed-industries/zed/commit/eb354c8d504071bdb79110a7a5c9d374c2864113)
changes Wayland to demand-driven rendering. It changes the platform frame API,
effect flushing, dirty-window wakeups, throttling, presentation completion,
and scheduling of callbacks queued during a frame across eight files. Its
patch also assumes newer frame-invalidator wakeup code not present in this
base. Replaying only its Wayland portion could park a dirty window indefinitely;
replaying it fully would be a cross-platform frame-lifecycle migration.
Defer it until that lifecycle can be tested across active/inactive windows,
resize, animation, occlusion, and GPU recovery. The focused crash fixes above
do not require it.

The November audit also leaves these migrations separate:

- Apple's objc2, `gpui_apple`, macOS 12/SDK 26, and `CAMetalDisplayLink`
  migrations, including the M5 frame-pacing fix, require a macOS build/runtime
  environment. The existing immortal display-link backport stays in place.
- Frame-limit, power/thermal, Linux power-monitoring, and demand-driven frame
  changes share the deferred frame-lifecycle prerequisites.
- WGPU 30, the Vulkan surface-teardown dependency update, and one-time surface
  configuration are not replayed onto the local WGPU 29 fork. They require a
  coordinated renderer/dependency update and GPU-recovery testing. WebGL
  reconfiguration changes target an unshipped browser backend.
- UAX #14 wrapping, font-resolution caching, Taffy 0.15, text-layout ownership,
  focus-cleanup indexing, sprite grouping, hot reload, iOS/OHOS, credentials,
  and `ScrollWheelEvent` renaming are API, feature, or performance migrations,
  not prerequisites for these crash/correctness backports.
- Synthetic Windows mouse-move suppression changes input semantics and awaits
  Windows runtime checks, including first entry at the origin, drag, repaint,
  and leave/re-entry behaviour.
- The `util` path/shell changes and `http_client` asset-download removal do not
  have matching Nucleotide consumers. `cargo tree -i util` finds no such
  package. Scheduler's sendable foreground executor is for new web HTTP
  clients; the GPUI macro change accompanies a proptest dependency upgrade.
  Neither is needed here. `collections`, `sum_tree`, `refineable`,
  `gpui_shared_string`, `gpui_tokio`, `gpui_util`, `http_client_tls`,
  `reqwest_client`, `media`, `util_macros`, and the logging/tracing companions
  have no changes between the two audit revisions. Existing local fixes stay.

## Validation boundaries

### November follow-up validation

The checkout started at parent local `main`
`b75604f1d1fd83841644f5dc01e2c9eb2d5f27dc`; fetched `origin/main` matched it.
Validated in a Linux orb with Rust 1.96.0:

- `cargo test --all --locked`, `cargo check -p nucleotide --locked`,
  `cargo build -p nucleotide --locked`, and `cargo build --workspace --locked`
  pass. The parent integration will rerun workspace tests/builds after combining
  the independent vendor updates.
- All 207 `gpui`, 22 `gpui_linux`, and 19 `gpui_wgpu` library tests pass in a
  temporary vendor workspace with the root lockfile, upstream test fonts, and
  `gpui/test-support`. This avoids changing the application's dependency graph.
  Regressions cover native close during an app borrow, modifier timeouts,
  executor-clock animation, text-system release, cached opacity, and Wayland
  serial arrival order/wraparound.
- Removing the opacity production backport makes its regression fail with
  actual opacity 1.0 instead of 0.5. Restoring it passes, including hover,
  leave, and entry caused by expanding under a stationary pointer.
- A native Wayland/Mesa fixture renders a cached child at full and half opacity,
  restores full opacity on hover, and dims again on leave. Inspected screenshots
  and a pointer-interaction recording show all four states. Overlapping 50%
  cyan WGPU paths match a 75% reference quad, checking destination alpha.
- `cargo check -p gpui_windows --tests --target x86_64-pc-windows-gnu --locked`
  passes. The DirectWrite regression covers mixed font runs, multibyte text,
  leading/repeated/trailing Unicode separators, baseline and cursor positions.
  Source inspection confirms that cursor, forced-update, and both GPU-loss
  message senders supply the validation number. This is compile/source coverage,
  not Windows runtime or HLSL validation.
- `cargo fmt --all -- --check`, formatting checks of every edited vendor Rust
  file, `git diff --check`, and `scripts/check-layering.sh` pass.

The fixture does not verify compositor acceptance of clipboard/activation
serials or overlapping activation requests. Windows DirectWrite, DirectX,
accelerators, cursor changes, and GPU recovery still need runtime checks.
macOS changes were neither compiled nor run; AppKit allocation failure,
Spanish-ISO input, and Metal blending need validation on macOS. X11 rendering
was not rechecked in this follow-up. These are limits, not claims of equivalence
across platforms.

### Previous audit validation

Validated in a Linux orb with Rust 1.96.0:

- `cargo check -p nucleotide --locked` and `cargo build -p nucleotide --locked`
  pass.
- `cargo check -p gpui_windows --tests --target x86_64-pc-windows-gnu --locked`
  passes with MinGW installed. This checks Rust sources, not HLSL compilation
  or Windows runtime behaviour.
- All 21 `gpui_linux` tests and 18 `gpui_wgpu` tests pass in a temporary copy
  of the vendor workspace, seeded with the root lockfile and with
  `gpui/test-support` enabled. Cargo resolves the standalone test dependencies
  there without changing Nucleotide's lockfile. The trimmed vendor workspace's
  retained upstream lockfile cannot be used with `--locked` directly.
- Negative controls reproduce `Task polled after completion` when the original
  failed-send behaviour is restored, and `texture must exist` when the
  released-texture guard is removed. Both regressions pass with the fixes.
- The HLSL transfer functions independently match expected values at 0.5:
  sRGB → linear is 0.217637641 and linear → sRGB is 0.729740053. A round-trip
  check alone would not detect the swapped exponents.
- `cargo fmt --all -- --check`, formatting checks of edited vendor Rust files,
  and `scripts/check-layering.sh` pass.
- Native Wayland/Mesa rendering shows the split editor, file-tree glyphs,
  borders, and status strip. X11 under the orb's headless Xwayland compositor
  shows an unpainted client area both before and after these backports; its
  rendering is not verified. The comparison does not establish the underlying
  cause. Missing language servers also limit this smoke run, not GPUI rendering.

GPU-backed atlas regressions cover both a released slot and an index invalidated
by clearing the atlas. Dispatcher tests cover the failed timer-send shutdown path.
Four temporary source-extracted IME probes use real Wayland client proxies over
a Unix socket to verify unchanged/quantized rectangle suppression, changed
rectangles, uncommitted state, and composing/pre-edit guards. The probes are
removed after validation; no fake-sink tests or test-only production abstractions
remain. These probes do not establish compositor or input-method runtime behaviour.

Windows runtime checks still require DirectWrite, DirectX, and real mixed-DPI
monitors. macOS runtime checks still require AppKit, CoreVideo, and Metal,
including native menu tracking and display reconfiguration. A Linux build is
not evidence that those platform-specific paths run correctly. macOS compilation
was not checked in this orb.
