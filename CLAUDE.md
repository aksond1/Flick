# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Flick is a region screenshot and screen recording tool for Hyprland (Wayland), written in Rust with GTK4. X11 and xdg-desktop-portal are out of scope for now, but capture sits behind a trait so they can be added.

Flow: `flick` freezes the screen, the user drags a region, and a six-button toolbar appears (Settings, Lock, Play, Edit, Copy, Download).

## Status

Work follows staged milestones:

- **0, 1 (done):** capture, frozen overlay, selection, Copy, Download, Lock, Esc.
- **2 (next):** Play starts recording (unfreezes first), Play again stops and opens a viewer (playback, save, copy file). Launching `flick` again while recording must also stop it. Measure audio/video offset (target under 100 ms).
- **3:** Edit window: brush (width, color), text (font, size, color), undo, redo, clear (undoable), copy, save.
- **4:** Settings window; GIF/WebM export. The format is a setting applied on save; recording always goes to a temporary MP4.
- **5:** multi-monitor and fractional scaling, 60 FPS, notifications, PKGBUILD, keybinding docs.

The recording core in `src/record/` is already implemented and reachable through `flick record`, but not wired to the UI. Settings, Play and Edit are disabled in `ui/toolbar.rs`. Microphone and hardware encoding are intentionally excluded.

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

System dependencies: `gtk4`, `gtk4-layer-shell`, `ffmpeg` (with `ffprobe` for tests), `wl-clipboard`, `pipewire-pulse`; `gst-plugin-gtk4` is for the stage 2 viewer. Integration tests run real ffmpeg with a fake capture backend, so they need no display or audio.

## Architecture

- **Two Wayland connections.** GTK owns one; `capture/wlr.rs` (`wlr-screencopy` v3) opens its own so it can run on a separate thread. `app.rs` must grab the frame *before* the overlay is mapped, or the overlay ends up in the picture.
- **Coordinates.** Regions are logical pixels relative to one output. Conversion to frame pixels goes through `Rect::scaled`, which rounds edges rather than sizes. `Frame` is always tightly packed BGRX; stride, y-inversion and channel order are normalized in `wlr.rs`.
- **Recording pipeline.** The capture thread puts frames into `FrameSlot` (newest frame only) using `copy_with_damage`, so a static screen produces nothing. `pacer.rs` turns that into constant frame rate by writing the latest frame on a monotonic clock to ffmpeg's stdin. ffmpeg captures system audio itself (`-f pulse -i @DEFAULT_MONITOR@`, which must precede the video input).
- **Stopping a recording.** The pacer drops stdin (EOF, `-shortest`), then `Encoder::finish` escalates to SIGINT and SIGKILL on timeout. Never join the pacer unconditionally: it may be blocked writing to a hung ffmpeg.
- **UI.** `ui/selection.rs` holds the selection rules with no GTK so they can be unit-tested; `ui/overlay.rs` is the layer-shell window. Closures hold `Weak<Overlay>`; the close handler owns the only lasting strong reference. The toolbar must stay strictly outside the region (`place_panel`) so it is not recorded; when nothing fits, it goes inside for screenshots and must be hidden while recording.
- **Click-through (needed in stage 2).** Set the surface input region from the surface's `layout` signal, so it is reapplied whenever the surface is laid out.
- **Clipboard** goes through `wl-copy`, whose forked process keeps the content alive after Flick exits.
- **Lock** (`~/.local/state/flick/lock.toml`) is valid only if the output's name, size and scale all still match. Config is `~/.config/flick/config.toml`.
- **Single instance** comes from the `GtkApplication` D-Bus id `dev.flick.Flick`.

## Testing the UI

Do not inject input into the user's live session. Run a nested `Hyprland -c <config>` with `HYPRLAND_NO_SD_VARS=1 HYPRLAND_NO_SD_NOTIFY=1` under `dbus-run-session`, and run Flick there with `GTK_A11Y=none GDK_DEBUG=no-portals` and private `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME`. Also use a private `XDG_RUNTIME_DIR` (a short path, e.g. `mktemp -d /run/user/$UID/fk.XXXXXX`, because socket paths are limited to about 108 bytes) and pass the host socket as an absolute path in `WAYLAND_DISPLAY`: a private D-Bus alone let the accessibility service overwrite the real session's `at-spi` socket.

The nested window is tiled by the host, so its output size and scale vary. Wait about 12 s after start, read the size from `flick outputs` and pass it as `VPTR_SIZE`; pointer coordinates are then physical pixels.

`examples/vptr.rs` (virtual mouse) and `examples/vesc.rs` (Escape key; takes a file from `xkbcli compile-keymap`) drive it. Hyprland 0.57 parses its config as Lua, so `.conf` syntax such as `monitor=` fails. Wait a second or so after a click before taking a screenshot, since the UI may not have redrawn.
