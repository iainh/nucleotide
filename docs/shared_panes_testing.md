# Shared-pane verification contract

The [migration plan](shared_panes_plan.md) defines policy and ownership. This
document defines observable acceptance, evidence categories and reproducible
checks. It does not claim that the mixed-pane implementation or native platform
checks are complete.

## Baseline and test modules

Start at `b23aaced1fcf2eb9ca220d7364d135e051c9803e`, not the older remote
`origin/main`. Source-reported baseline: Linux, 2,257 passed, 0 failed and
11 ignored across 50 test/doc-test suites, with performance tests excluded.

Reproduce that selection using the pinned Rust 1.96.0 and Zig 0.17.0 tools,
repository Linux dependencies and locked dependencies:

```sh
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace --locked -- --skip tests::performance_tests --skip tests::integration_tests::tests::performance_tests
```

This is not an unfiltered full-test result: report excluded performance tests.
CI uses this same workspace-wide selection on Linux x86_64, macOS ARM64 and
Windows x86_64. Linux remote fixtures and ARM compilation remain separate gates.
Adding a workflow job is not evidence that GitHub has executed it.

Dedicated files owned by H:

- `crates/nucleotide/src/application/shared_pane_contract_tests.rs`: baseline
  public command/keymap behaviour, nested under the existing application test
  harness by G so private fixture helpers need not become public. It preserves
  no-argument placement-close aliases separately from explicit buffer-targeted
  all-copy close. Hosted zero-slot maintenance requires the delivered bridge.
- `crates/nucleotide/src/workspace/shared_pane_scenario_tests.rs`: mixed-pane
  scenarios against G's delivered interfaces. The early real `PaneHost` seam
  covers two document slots plus an empty content-only pane, focus disarming,
  nonroot document-pane collapse and final zero-slot projection without quit or
  reflow. A second real-editor scenario covers dirty buffer-close validation and
  forced all-copy removal with unrelated document/content panes retained. A
  third covers split-state copying, dedup target-state precedence, final-tab
  transfer before source-slot release and retained undo history. It seeds
  distinct selections, scroll anchors and jumplists to distinguish the owners.
  These are not terminal/image renderer tests. See the integrated execution
  record below for the tested revision and remaining failures.

G owns registrations. A file that is not registered is not an executed test.
Do not add placeholder mixed-pane tests, mock implementations or guessed APIs.
Existing `application/tests.rs` already protects inactive-tab drag/release,
shared dirty-copy close, directory scratch preservation and focus restoration.
Keep those tests; add complementary cases rather than copies.

Characterizations should enter through real Core/GPUI workspace input, command
prompt, tab hitboxes or published controller operations. Assert document text,
placements, focus, process identity and output, not private helper call order.
If a baseline quirk intentionally changes, record both contracts rather than
forcing the old editor-backed behaviour onto content-neutral panes.

### Preserved command compatibility

The Core/GPUI baseline distinguishes `sp`/`hsplit` and `vs`/`vsplit` with quoted
file arguments. Its GUI normalizer recognizes `split` only without arguments;
`split <path>` is not an established baseline capability. Preserve/extend aliases
explicitly in the typed command adapter rather than assuming this was tested.

The GUI intercepts no-argument `bc`/`bclose`/`buffer-close`/`close` as placement
close: it removes a shared copy, then prompts on the final dirty copy. Preserve
that compatibility, including force handling. `CloseFile`/tab-close also removes
only the selected placement. An earlier proposal to make no-argument GUI
`bclose` unload everywhere was withdrawn; tests must not choose that product
change implicitly.

Explicit buffer-targeted Helix operations such as `bclose '<buffer-path>'`
validate and unload the affected buffer everywhere. Refusal preserves all
copies; force removes all affected copies but not unrelated content, and does
not mean pane-close or application quit. Empty nonroot panes may collapse under
the ordinary layout policy. Final integration tests exercise this distinction
through the GUI as well as the editor/host seam.

## Automated acceptance matrix

| Area | Required observations | Distinguishing inputs/failure cases |
| --- | --- | --- |
| Model topology | Four edge splits, nested same/opposite axes, ordered memberships, active/MRU/pinned/preview state, valid collapse | Ordinary equal sibling redistribution versus target-only splitting with unequal weights |
| Moves | Reorder, cross-pane transfer, target deduplication, final-tab atomic move/collapse | Existing target copy, inactive dragged tab, final source tab and no-op self move |
| Geometry/navigation | Gap-free logical-pixel allocation, finite positive weights, deterministic neighbours, swap/transpose, resize minima | Asymmetric splits, fractional bounds, tiny windows, NaN/infinite input and ties |
| Model sequences | Generated operation sequences preserve topology/membership invariants | Check invariants after each real operation, not random inputs that mostly reject |
| Editing state | Selection, independent cursor, scroll/wrap, jumplist and pending edits survive split/move | Different offsets in two copies; retained empty and modified scratch; preview promotion/replacement |
| Commands | `sp`/`split`/`vs`, arguments, static fallback; `hsplit[_new]`/`vsplit[_new]`, directional/next/previous navigation, rotation, swaps, transpose, `wclose`/`wonly` | Split to another file versus current document; counts, registers, pending prefixes, mapped lists and macros |
| Ordered continuation | Host split → host focus → edit modifies only the final document | Move to content-only pane, failed host operation, cancelled/suspended macro: hidden document unchanged |
| Completion boundaries | Synchronous viewport command finishes its mapped edit without another key; delayed navigation releases only its matching continuation | Close A after visiting B → terminal → A: B unchanged and tail cancelled. Hold a real definition response, click/type in C, then release it: C unchanged and focus retained; a second unchanged-origin request must complete its edit. |
| Close/quit | `q`/`q!`/`qa`/`qa!`/`wq`, buffer-close/all/others, explicit pane close versus fallback | Dirty shared copy versus last copy, cancelled and accepted validation, write failure before close |
| Open destinations | CLI, native open, picker, LSP goto, remote open, resource edits and delayed jobs preserve intent | Split versus replace, missing/stale source pane/view, callback after project replacement |
| Last editor | Terminal/image remains visible and maintenance continues after last editor presentation closes | Zero Helix views is not application quit; ordinary last-tab close versus explicit final-pane quit |
| Retained slots | Every real editing slot has one workspace owner; inactive document state survives transfer | Repeated image I → open B, I → open C in A's pane: one owned slot, A selection/offset retained. Transfer inactive A while displaying B into C's pane: A state copied, B/C state unchanged, existing target A wins dedup. |
| Images | No fake document/view, independent duplicate zoom/scroll, move transfers state, local/remote cache | Hidden lazy placement retains resource; final layout placement release permits eviction |
| Dock | Same model/renderer, stable placement on hide/reveal, remembered size | Toggle versus new session, emptied dock creates new session, runnable does not replace another session |
| Input/focus | Actual focused content owns keys; overlays restore initiating/surviving placement | Shell Ctrl-W and pending editing continuation, removed initiator, completion/hover/IME on non-document switch |

Model owners keep their own invariant/property tests. H's integration scenarios
exercise public behaviour across ownership boundaries; they do not reimplement
the model's private algorithms or snapshot incidental action ordering.

### Asymmetric model acceptance cases

These cases were identified in oracle review; review is not execution evidence.
A owns the real model regressions and reports their outcomes before cutover.
`H` below means a horizontal arrangement of children; `V` means vertical.

| Case | Independent expected result |
| --- | --- |
| `H[A,H(B,C),D]`, width 600, per-leaf minimum 100 | Outer children each allocate 200. Increasing the first divider by 100 clamps to zero: the B/C subtree is already at its propagated minimum 200. D stays 200; rendering and resize use the same subtree minima. |
| `H[A,V(B,C)]`, transpose focused A | Only the root axis changes to V. The B/C subtree remains V; no recursive transpose. |
| Sole-tab self-edge drop | All edges in main, nested and dock layouts are no-ops: same PaneId, placement and activation history. |
| Configured `LeftNeighbour` close | Closing the active middle tab chooses its left neighbour even when MRU or right-adjacent policy would choose another tab. |
| Dock hide/reveal with multiple tabs | Restore the remembered dock pane/tab and actual input focus, not just the visible flag or the last central pane. |
| Directional navigation | Compare unequal-pane cases against baseline Helix top/left selection before changing to candidate-centre ranking; policy remains pending integration's preservation decision. |

For generated runs, report seeds/sequences and transitions separately. For
example, 32 sequences × 300 steps is 9,600 transitions, not 9,600 independent
sequences. A transition count does not establish execution unless accompanied by
the command, revision and actual result.

## Real PTY and run-lifecycle matrix

These checks require real native pseudoterminals. Headless emulator tests alone
do not satisfy them.

| Scenario | Required evidence |
| --- | --- |
| Two shells plus one task | Distinct session IDs and child PIDs, independent input/output and per-run association |
| Unequal visible sizes | Rows/columns reported inside each child, not just dimensions stored in a view model |
| Repeated pane/window resize | Each child receives its own size; no zero-size resize during transfer; hidden session keeps last nonzero size |
| Move dock ↔ centre | Same child PID/start marker, scrollback, selection and input channel before/after |
| Hide then reveal | Output produced while hidden survives; no termination or restart |
| Spawn races | Close during environment resolution/remote spawn prevents late mount; queued initial input does not reach another session |
| Failure/exit | Failure before mount and natural hidden exit retain details/output; unknown status is completed, not fabricated success |
| Explicit close/shutdown | Targeted termination, duplicate exit idempotence, cleanup without killing unrelated sessions |
| Inactive task | Correct run/activity receives completion exactly once; output and exit status remain available |
| Remote paths | Service-owned PTY and proxy fallback, mapped cwd, resolved environment and initial input, disconnect/reconnect and project replacement |

Do not replace these observations with mock channel interactions. Deterministic
controller tests may cover lifecycle transitions, but label them as controller
tests rather than native PTY evidence.

## Same native GUI scenario on every platform

Use one exact integrated commit for all platforms. Obtain consent before using
separate platform runners. Record display backend and scale; do not infer them
from the OS name. Record pointer/keyboard interactions and retain child identity
and size logs alongside targeted screenshots.

1. Open two different documents and an image.
2. Start two terminals with identifiable long-lived child processes.
3. Tile all content kinds. Reorder an inactive tab; activation occurs only on
   release, not mouse-down or drag start.
4. Drag terminals between panes and dock. Compare child IDs/start markers before
   and after; moving must not restart a process.
5. Resize nested unequal splits and the native window. Query dimensions inside
   each child; verify document cursor/pointer alignment and overlay anchors.
6. Use editor pane commands to focus terminal and image panes.
7. Type terminal control keys, including Ctrl-W. Check shell behaviour and verify
   hidden document text remains unchanged.
8. Exercise completion/hover, command prompt, modal unsaved confirmation,
   cancellation, IME and focus restoration, including a removed initiating pane.
9. Close the last editor presentation while a terminal remains. Verify ongoing
   output/maintenance; then test close, hide/reveal and explicit application quit.
10. Finish a task while its tab is inactive. Verify retained output and the exact
    run's completion/status, without affecting another terminal.

| Platform | Additional native checks | Initial evidence status |
| --- | --- | --- |
| Linux Wayland | Clipboard/primary selection, local shell, remote-service PTY | Source reports prior baseline native interaction; new mixed scenario not run |
| Linux X11 | Same scenario and selection/focus checks where supported | Not run for this migration |
| macOS Apple Silicon/Retina | Logical versus physical pixels, Command shortcuts, IME, titlebar/resize | Not run; runner access requires consent |
| Windows x64/ConPTY | PowerShell and another supported shell, DPI changes, IME, drive paths, WSL remote terminal | Not run; runner access requires consent |

Windows ARM terminal tests and Linux ARM compilation remain useful separate
results. Neither proves x64 ConPTY/WSL or native GUI interactions. A Linux GUI
run cannot stand in for Retina, Windows DPI or IME behaviour.

## Quality gates and result recording

Run the applicable gates with repository prerequisites:

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo check --workspace --all-targets --locked
./scripts/check-layering.sh
cargo deny check
cargo +nightly-2026-07-01 udeps --all-targets --workspace
```

The unfiltered workspace command is the plan's full-test gate. If only the CI
selection was run, report it as such. Use the pinned CI versions of auxiliary
tools; do not install an unpinned replacement merely to obtain a green result.
Do not trigger workflows or deployments as part of local verification.

For each execution record:

| Field | Record |
| --- | --- |
| Revision | Full integration SHA; identify any local patch or registration-only harness |
| Environment | OS, architecture, native runner/orb, display backend and scale if applicable |
| Invocation | Exact command or numbered GUI scenario and fixture inputs |
| Outcome | Passed/failed/ignored/skipped, explicit exclusions and reason |
| Evidence | Test output; inspected screenshots/recording for interaction; child PID/size/output logs for PTY claims |

Compilation establishes type/build compatibility. Unit/controller tests establish
the exercised public contracts. Headless GPUI tests exercise their simulated
input and state, not native compositor, IME or shell behaviour. Screenshots show
arrangement; recordings show exercised interaction; child-process logs establish
PTY identity and actual dimensions. Keep these claims separate and leave every
unperformed check marked unperformed.

## Verification owner's initial execution record

On Linux x86_64 in an orb, an isolated worktree at
`b23aaced1fcf2eb9ca220d7364d135e051c9803e` added only a test registration and
four characterization scenarios. No hosted implementation was added to that
baseline. The original command-adapter tests passed independently: 7 passed,
0 failed. The new characterization selection passed: 4 passed, 0 failed.

The workspace-wide invocation was:

```sh
CARGO_TARGET_DIR=/home/user/workspace/repo/target CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace --locked -- --skip tests::performance_tests --skip tests::integration_tests::tests::performance_tests
```

Result: **2,261 passed, 0 failed, 11 ignored across 50 test/doc-test suites**.
This independently reproduces the source baseline plus four retained
characterization cases, including the preserved GUI `bclose` placement-close
contract. It does not validate the migration implementation or the additional
explicit buffer-targeted test added after that run.

After correcting the compatibility contract, the same baseline harness ran
`cargo test -p nucleotide --locked --lib shared_pane_contract_tests` with the
same profile/target environment: **5 passed, 0 failed**. This includes both the
no-argument GUI placement-close case and the explicit buffer-path all-copy
validation/force case. The hosted maintenance case was excluded from this
pre-hosted harness, not reported as passing.

Workspace formatting, direct `rustfmt --check` on both unregistered test files,
`git diff --check` and a local CI YAML parse/structural check passed. No GitHub
workflow was triggered. The hosted zero-view maintenance test and real PaneHost
scenarios require G's integrated revision; they were not part of this baseline
execution. Native GUI/PTY/platform checks above remain separate and unperformed
by this owner.

## Integrated pre-fix regression record

H executed the real integrated revision
`a9f62fb006952ea3bf41ea1c57abe2fbf98d368e` in an isolated Linux x86_64 worktree.
The only implementation-file delta was G's 23-line `cfg(test)` setup/observation
API; the two dedicated scenario files contained the new regressions. No runtime
implementation fix was applied.

```sh
CARGO_TARGET_DIR=/home/user/workspace/repo/target CARGO_PROFILE_DEV_DEBUG=0 cargo test -p nucleotide --locked shared_pane -- --nocapture
```

The original nine shared-pane tests passed. Six new tests failed with distinct
observable results:

| Regression | Pre-fix observation |
| --- | --- |
| Targeted close followed by mapped deletion | A was removed, but B changed from `B must remain intact` to ` must remain intact`. |
| Viewport followed by mapped deletion | A remained `first\nsecond\nthird` before any later key, instead of becoming `first\nsecond\nhird`. |
| Real delayed definition request | After the server observed the JSON-RPC request but before releasing its response, a click and `h` in C changed `charlie` to `charli`. |
| Image-active document open | Two real Helix slots existed but only one appeared in the workspace's ownership map. |
| Inactive document transfer | A's destination selection was the default 0–1, not its retained asymmetric 83–77 selection. |
| Final quit after closing the dock's last terminal | `q` emitted no `ShouldQuit`; the allocated empty hidden dock prevented final-pane quit. |

Result: **9 passed, 6 failed, 0 ignored**. These failures confirm the missing
contracts, not successful migration acceptance. The delayed-definition fixture
uses the real LSP registry/client and duplex JSON-RPC transport, not a callback
mock. It also includes a matching-response positive control for the fixed
implementation; that later branch is not reached by the pre-fix run. G owns the
runtime fixes and the after-fix integrated rerun. No native GUI or PTY result is
implied by these headless tests.
