# Flick

Region screenshots and screen recording for Hyprland (Wayland), written in Rust with GTK4.

Everything happens inside the selected region: selecting, editing, recording and playback, with no separate windows.

> Work in progress. Screenshots, recording, preview and the editor are ready. There is no settings UI yet; options are set in the config file.

## Features

- **Selection:** the screen freezes; move the region or resize it with 8 handles or their dashed lines.
- **Lock:** pins the region, so the next launch opens with it right away.
- **Screenshots:** copy to the clipboard or save as PNG.
- **Recording:** video with system audio; the region can be moved while recording.
- **Preview:** the video plays inside the region, with seeking, volume, saving and copying the file.
- **Editor:** brush and text, any color, font, size, undo, redo, clear.

## Installation

Dependencies (Arch Linux):

```sh
sudo pacman -S --needed gtk4 gtk4-layer-shell gst-plugin-gtk4 gst-plugins-base \
  gst-plugins-good gst-libav ffmpeg wl-clipboard pipewire-pulse
```

Building also requires Rust (edition 2024).

```sh
cargo build --release
./target/release/flick
```

It is handy to bind this to a key in your Hyprland config.

## Usage

Run `flick`, select a region with the mouse and choose an action on the toolbar:

| Button | Action |
|---|---|
| Settings | Not implemented yet |
| Lock | Pin or unpin the region |
| Play | Start recording; press again to stop and preview |
| Edit | Open the editor |
| Copy | Copy the image and close |
| Download | Save the image and close |

| Key | Action |
|---|---|
| Esc | Close; in the editor, finish the text or leave the editor |
| Space | Pause or play during preview |
| Ctrl+Z | Undo (editor) |
| Ctrl+Y, Ctrl+Shift+Z | Redo (editor) |

- Launching `flick` again while recording stops it. This is the only way when the region fills the screen and there is no room for the toolbar.
- While recording, move the region with the dotted grip on the toolbar; it cannot be resized.
- Editor annotations are included by Copy and Download, but not in videos.

### Headless

```sh
flick outputs
flick shot --region 0,0,400,300 --file a.png    # without --file: clipboard
flick record --region 0,0,400,300 --seconds 5 --file a.mp4
```

## Configuration

`~/.config/flick/config.toml` (every field is optional):

```toml
save_dir = "/home/me/Pictures/Flick"  # default: Flick in your pictures directory
fps = 30
audio = true          # system audio
cursor = true         # pointer in videos
video_format = "mp4"  # mp4, webm or gif; applied when saving
```

## Limitations

- Requires a compositor with `wlr-layer-shell` and `wlr-screencopy`; tested only on Hyprland. X11, GNOME and KDE are not supported.
- No microphone and no hardware encoding (video is encoded with x264).
- Multiple monitors and fractional scaling are not fully tested yet.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Architecture and UI testing are described in [CLAUDE.md](CLAUDE.md).
