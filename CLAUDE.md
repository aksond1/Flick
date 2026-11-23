# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Flick is a region screenshot and screen recording tool for Hyprland (Wayland), written in Rust with GTK4. X11 and xdg-desktop-portal are out of scope for now, but capture sits behind a trait so they can be added.

Flow: `flick` freezes the screen, the user drags a region, and a six-button toolbar appears (Settings, Lock, Play, Edit, Copy, Download).

## Status

Work follows staged milestones:

- **0-3 (done):** capture, frozen overlay, selection, Copy, Download, Lock, Esc; recording with system audio (stop by the button or by launching `flick` again) and an in-region preview; an in-region editor (brush, text, undo, redo, clear).
- **4:** Settings (planned in place, like the rest); GIF/WebM export: `record/export.rs` works, progress display is missing. The format is a setting applied on save; recording always goes to a temporary MP4.
- **5:** multi-monitor and fractional scaling, 60 FPS, notifications, PKGBUILD, keybinding docs.

Settings is disabled in `ui/toolbar.rs`. The config file already drives FPS, audio, cursor and the saved video format. Microphone and hardware encoding are intentionally excluded.

User decisions: no separate windows (preview and editor were moved into the region), blue accent `#1a73e8`, no resizing while recording. Commits go to stage branches (`stage-N`).

## Commands

```sh
cargo build --release
cargo run                                   # open the overlay
cargo run -- outputs                        # headless: list outputs
cargo run -- shot --region 0,0,400,300 --file a.png   # without --file: clipboard
cargo run -- record --region 0,0,400,300 --seconds 5 --file a.mp4
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --lib geometry                   # one module
cargo test --test record encoder_failure    # one integration test
```

System dependencies: `gtk4`, `gtk4-layer-shell`, `ffmpeg` (with `ffprobe` for tests), `wl-clipboard`, `pipewire-pulse`; `gst-plugin-gtk4` (preview playback). Integration tests run real ffmpeg with a fake capture backend, so they need no display or audio.

## Architecture

- **Two Wayland connections.** GTK owns one; `capture/wlr.rs` (`wlr-screencopy` v3) opens its own so it can run on a separate thread. `app.rs` must grab the frame *before* the overlay is mapped, or the overlay ends up in the picture.
- **Coordinates.** Regions are logical pixels relative to one output. Conversion to frame pixels goes through `Rect::scaled`, which rounds edges rather than sizes. `Frame` is always tightly packed BGRX; stride, y-inversion and channel order are normalized in `wlr.rs`.
- **Capture loop.** A busy Wayland socket (`WouldBlock`) is not an error, and events carry a frame id so those of a cancelled capture are ignored; both once ended recordings. Streaming is paced by `CaptureRequest::interval` to spare the compositor.
- **Recording pipeline.** The capture thread puts frames into `FrameSlot` (newest frame only) using `copy_with_damage`, so a static screen produces nothing. `pacer.rs` turns that into constant frame rate by writing the latest frame on a monotonic clock to ffmpeg's stdin. ffmpeg captures system audio itself (`-f pulse -i @DEFAULT_MONITOR@`, which must precede the video input).
- **Stopping a recording.** The pacer drops stdin (EOF, `-shortest`), then `Encoder::finish` escalates to SIGINT and SIGKILL on timeout. Never join the pacer unconditionally: it may be blocked writing to a hung ffmpeg.
- **UI.** `ui/selection.rs` holds the selection rules with no GTK so they can be unit-tested; `ui/overlay.rs` is the layer-shell window. Closures hold `Weak` references; `Overlay` keeps itself alive through its `keep` field until closed and owns the `Preview`. The toolbar must stay strictly outside the region (`place_panel`) so it is not recorded; when nothing fits, it goes inside for screenshots and must be hidden while recording.
- **Recording overlay.** The same window turns see-through (`Overlay::set_live`): frozen picture removed, no keyboard, input region limited to the panel and reapplied on every surface `layout`. Recording starts after `UNFREEZE` so the frozen frame is not captured.
- **Moving while recording.** The panel grip moves the region (its inside must be click-through). The border follows the pointer exactly, but the screen shows it some frames late, so during a drag (`Live::Track`) the capture takes a wider window, finds the border in the picture among the positions recently drawn (`capture/track.rs`) and cuts out its inside; frames without a match are dropped. The overlay must therefore draw the border with the constants from `track.rs`. A lagging border felt jerky and pausing the video lost the movement; both were tried. The panel turns transparent meanwhile to avoid per-frame relayout. Resizing while recording was deliberately removed. `record/fit.rs` absorbs one-pixel size changes caused by fractional scaling.
- **Reentrancy.** Hiding a widget can synchronously end a gesture, so never hold a `RefCell` borrow that a gesture handler needs while changing widgets.
- **Preview.** After stopping, the overlay plays the recording inside the region (`ui/preview.rs`); clicks are accepted only in the region and the panel, the rest of the screen stays usable. Ask questions inline: dialogs would open underneath the overlay. Recordings live in `~/.cache/flick` until the preview closes. Copy saves first, because the clipboard holds a file URI that must outlive the temporary file. Launching `flick` during a preview opens a new overlay.
- **Editor.** Edits in place: `ui/editor/document.rs` holds operations in frame pixels and one cairo function draws both screen and export. Annotations belong to the frozen frame, stay after leaving the editor and are included by Copy and Download. Only popovers work on the overlay (dialogs open underneath), hence the own color picker and font list. Brush and text have separate colors. Escape or a press elsewhere finishes the text being typed, never discards it; undo takes it back. Undo and redo use hardware key codes so they work in any keyboard layout. The text being typed is a `TextView` sized by hand to match the pango rendering. `Stepper` replaces `SpinButton`, whose layout cannot be made compact, and both tools use equally wide controls so the panel keeps its size. Popover key events do not reach the window's key controller.
- **Styling** is one CSS string in `ui/overlay.rs`, scoped under `.flick-toolbar`.
- **Clipboard** goes through `wl-copy`, whose forked process keeps the content alive after Flick exits.
- **Lock** (`~/.local/state/flick/lock.toml`) is valid only if the output's name, size and scale all still match. Config is `~/.config/flick/config.toml`.
- **Single instance** comes from the `GtkApplication` D-Bus id `dev.flick.Flick`.

## Testing the UI

Do not inject input into the user's live session. Run a nested `Hyprland -c <config>` with `HYPRLAND_NO_SD_VARS=1 HYPRLAND_NO_SD_NOTIFY=1` under `dbus-run-session`, and run Flick there with `GTK_A11Y=none GDK_DEBUG=no-portals` and private `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME`. Also use a private `XDG_RUNTIME_DIR` (a short path, e.g. `mktemp -d /run/user/$UID/fk.XXXXXX`, because socket paths are limited to about 108 bytes) and pass the host socket as an absolute path in `WAYLAND_DISPLAY`: a private D-Bus alone let the accessibility service overwrite the real session's `at-spi` socket.

The nested window is tiled by the host, so its output size and scale vary. Wait about 12 s after start, read the size from `flick outputs` and pass it as `VPTR_SIZE`; pointer coordinates are then physical pixels.

For audio, run private `pipewire`, `wireplumber -p policy` and `pipewire-pulse` in that runtime directory with a `module-null-sink` as the default sink, so the user's audio is never touched.

`examples/vptr.rs` (virtual mouse) and `examples/vesc.rs` (keyboard; takes a file from `xkbcli compile-keymap` and evdev codes, Escape by default) drive it. Hyprland 0.57 parses its config as Lua, so `.conf` syntax such as `monitor=` fails. Wait a second or so after a click before taking a screenshot, since the UI may not have redrawn.
