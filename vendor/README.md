# vendor/

Patched copies of GPUI crates: two that only build on macOS, and GPUI Kit's `gpui-base`. The root `Cargo.toml` points
crates.io at them with `[patch.crates-io]`. They are not workspace members, so this project's
fmt / clippy settings don't apply to them.

| Directory | Upstream | Source |
| --- | --- | --- |
| `gpui-pre-apple/` | `gpui-pre-apple 0.3.8` (GPUI Kit 0.7.1) | Metal renderer and sprite atlas |
| `gpui-pre-macos/` | `gpui-pre-macos 0.3.8` (GPUI Kit 0.7.1) | NSWindow / NSView, frame pacing |
| `gpui-base/` | `gpui-base 0.7.1` (GPUI Kit) | Input / editor engine (steady cursor) |

The first commit, "build(gpui): 原样引入 …", copies the crates.io sources as they are (without
`Cargo.lock` and `.cargo_vcs_info.json`). Every later change is marked with a `ZJ patch` comment;
`git diff <that commit> -- vendor` shows all of them. Why we patch, what it saves and how to
measure it are in [docs/adr/0005-gpu-memory.md](../docs/adr/0005-gpu-memory.md).

The 0.7.1 upgrade rebases the ZJ changes onto the published crates.io sources. Input
viewport intents coexist with upstream's horizontal alignment and textarea scroll clamps;
the macOS frame callback keeps upstream's signal timing and source while honoring surface
restoration and idle frame suspension. The renderer keeps upstream's shared-storage policy
and its debug-only screenshot configuration alongside ZJ's shared atlas and drawable limit.

## Our diff

`gpui-pre-apple`:

- `src/zj_low_memory.rs` (new): the policy, written without Metal types so it can be tested
  anywhere. The app crate includes it in its tests (`crates/app/src/main.rs`). It covers:
  - the `ZJ_GPU_LOWMEM` env toggle;
  - the drawable count;
  - when the path textures need (re)allocating and when they have been idle long enough to free;
  - what to release when a window is hidden.
- `src/metal_renderer.rs`:
  - `maximumDrawableCount` is 2 instead of 3.
  - The path intermediate texture and its MSAA texture are allocated by the first frame that
    draws a path, at that frame's size, instead of on every resize. They are freed after 10 s
    without paths.
  - `zj_release_surfaces` / `zj_restore_surfaces`: while the window can't be seen, the layer is
    shrunk to 1 × 1, which drops the pooled drawables, and the path textures are freed. When the
    window is minimized or the app is hidden, the layer's contents are cleared as well. Nothing
    is drawn while released.
  - `zj_trim_idle`: a visible window that has not drawn for 3 s resizes its layer away and
    back, which empties the drawable pool (the presented frame stays in the layer's contents),
    and frees the path textures (their own 10 s check only runs when a frame is drawn).
    The macOS window calls it on every display-link tick.
  - One sprite atlas is shared by all window renderers on the same device. It is held weakly,
    so it is freed with the last window.
  - Headless renderers keep upstream behaviour.
  - The shader library is compiled once per process. GPUI Kit always enables
    `runtime_shaders`, so upstream compiles the Metal source again for every window.
- `src/metal_atlas.rs`: polychrome atlas textures start at 512² (1 MiB) instead of 1024²
  (4 MiB). They only ever hold the welcome logo (320 × 300 device pixels) and small file
  icons; anything bigger still gets a texture of its own size. Monochrome (glyphs and SVG
  icons) keeps 1024².
- `src/gpui_apple.rs`: `pub mod zj_low_memory;`.
- `Cargo.toml`: `[lints.rust] warnings = "allow"`. Path dependencies don't get `--cap-lints`, and
  upstream prints about 1200 deprecation warnings.

`gpui-base` (GPUI Kit 0.7.1, the input and editor engine):

- `src/input/editor/mod.rs`, `src/input/base/kind.rs`, `src/input/base/element.rs`,
  `src/input/mod.rs`: one optional application-owned line-end annotation. It is
  positioned in prepaint using that frame's shaped logical line, after its last
  wrapped segment. Its measured width extends horizontal scrolling even when the
  endpoint is to the right of the viewport; vertically offscreen or folded lines
  are omitted. It does not enter the text, wrapping or selections. ZJ uses it for
  current-line blame; the application supplies the gap, style and width cap.
- `src/input/editor/mod.rs`, `src/input/base/state.rs`, `src/input/base/element.rs`:
  reviewed source positions use a one-shot cursor/viewport intent consumed by the
  first layout, with fresh wrapped-line geometry. Native frame callbacks run before
  drawing and cannot safely read the returned source's previous caret geometry.
  It replaces the ordinary deferred reveal before choosing the visible slice and
  clamps the painted offset to the current frame's scroll range at file boundaries.
  Later cursor movement, text revision, blur or explicit scrolling cancels the intent.
- `src/input/base/blink_cursor.rs`: the cursor is steady. `start` shows it without a timer and
  `pause` (every keystroke) only repaints when it was hidden; `stop` (blur) hides it as before.
  Upstream blinks every 500 ms, and each blink repaints the whole window: an idle focused
  editor then costs about 3% CPU, and the renderer never goes 3 s without a frame, so it keeps
  its spare drawable and path textures (about 190 MB instead of 67 MB, one window). There is
  no switch for it in Kit. The blink loop, its constants and its tests are removed (the
  file's own tests now describe the steady cursor; vendor crates are not workspace members,
  so they do not run in CI). `crates/app`'s `idle_ui_tests` checks that an idle focused
  editor leaves no timer.
- `src/input/editor/lsp/definitions.rs`: a ⌘-click with no definition found for that spot
  yet asks the definition provider right away and follows the answer (as F12 does), instead of
  being ignored. Upstream only follows a definition a ⌘-hover has already fetched, so a click
  without moving the pointer under ⌘, or before a slow answer (ZJ's symbol index still being
  built), did nothing. The click still places the cursor; the jump is dropped if the text
  changed or the editor lost the focus meanwhile.
- `Cargo.toml`: the same lint override.

`gpui-pre-macos`:

- `src/window.rs`:
  - `windowDidChangeOcclusionState` calls `zj_release_surfaces` when the window stops being
    visible. It passes `Hidden::Gone` when the window is miniaturized or `NSApp.isHidden`, and
    `Occluded` otherwise.
  - `step` (the display-link tick) calls `zj_trim_idle` after the frame callback, and once the
    window is idle (its spare drawable given back, no forced present pending) stops the display
    link.
  - `frame_waker` (GPUI calls it when an idle window has something to draw: a notify, an input,
    a next-frame callback) starts the link again. It uses `try_lock`; when the window state is
    held by one of its own callbacks, the wake runs on the main queue next.
  - When the window is visible again, it calls `zj_restore_surfaces` and sets `zj_force_present`.
    The next display-link `step` then passes `require_presentation: true`, so GPUI presents the
    last scene again even if nothing changed.
- `Cargo.toml`: the same lint override.

- `src/system_notifications.rs`: native notifications use the default notification sound,
  including while ZJ is frontmost. Submission waits for notification authorization; only the
  latest request for a tag can leave an authorization callback, and dismissal also cancels
  pending authorization requests. No requests are retained once the callbacks complete.

`ZJ_GPU_LOWMEM=0` switches all of this off at startup, for A/B measurements. `ZJ_FRAME_LOG=1` logs
every presented frame and every display-link stop / wake (`event=frame`, `event=display_link`),
to find what wakes an idle window.

## Checking on Linux

The crates only compile for macOS, and their build script only compiles the shaders on a Mac
host. To type-check them from Linux:

```sh
rustup target add aarch64-apple-darwin
cargo check --target aarch64-apple-darwin -p gpui-pre-macos   # fails: missing shader outputs
for d in target/aarch64-apple-darwin/debug/build/gpui-pre-apple-*/out; do
  touch "$d/stitched_shaders.metal" "$d/shaders.metallib"     # placeholders, check only
done
cargo check --target aarch64-apple-darwin -p gpui-pre-macos
```

For clippy, run the same check with `RUSTC_WRAPPER=$(rustup which clippy-driver)` and
`CLIPPY_ARGS=-Wclippy::all__CLIPPY_HACKERY__-Wwarnings__CLIPPY_HACKERY__` (the second flag
undoes the lint override above), and compare the warnings with the unpatched commit. The patch adds only `cocoa` deprecation warnings of the kind upstream already has.

## Upgrading GPUI

1. Copy the new crates.io sources over these directories.
2. Re-apply the `ZJ patch` hunks from `git diff`.
3. Update the version in this file and in the `[patch.crates-io]` comment.
4. If upstream has fixed the same problem, delete the directory and its `[patch]` entry.
5. Check that the low-memory behaviour survived, because a lost hunk still builds:
   - `cargo test -p workspace-editor idle_ui` passes (no timer left in an idle window; it
     fails with Kit's original blinking caret);
   - on a Mac, `ZJ_FRAME_LOG=1 target/dist/workspace-editor <folder>` prints
     `event=display_link state=paused` about 3 s after the last frame, and nothing more until
     you touch the window;
   - `python3 tools/measure_budget.py --breakdown` reports 0 frames and 0 wakes while idle,
     and footprints within the budget in CLAUDE.md.
