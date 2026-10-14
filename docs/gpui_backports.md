# GPUI backports

Nucleotide keeps its vendored Zed base at
[`51fac82ddd`](https://github.com/zed-industries/zed/commit/51fac82ddd60380b4ea56d9e02b895628742c454)
(22 June 2026). These focused backports avoid the `gpui_apple` migration and
Rust 1.98.1 requirement of the audited
[`93d7456688`](https://github.com/zed-industries/zed/commit/93d745668805029476e6d7fbb19652116fed51de).
The application still builds with Rust 1.96.0.

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

## Validation boundaries

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
