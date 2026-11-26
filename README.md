# Flick

Region screenshots and screen recording for Hyprland (Wayland), written in Rust with GTK4.

Everything happens inside the selected region: selecting, editing, recording and playback. Only the settings have a window of their own.

> Work in progress. Screenshots, recording, preview, the editor and the settings are ready.

## Features

- **Selection:** the screen freezes; move the region or resize it with 8 handles or their dashed lines.
- **Lock:** pins the region, so the next launch opens with it right away.
- **Screenshots:** copy to the clipboard or save as PNG.
- **Recording:** video with system audio and the microphone; the region can be moved while recording.
- **Preview:** the video plays inside the region, with seeking, volume, saving and copying the file.
- **Editor:** brush and text, any color, font, size, undo, redo, clear.
- **Three keybindings:** a main and an alternative behavior after selecting (copy, save, show the toolbar, in any useful mix) and fastshots of the locked region with no UI at all.
- **Settings window:** keybindings written into your Hyprland config, folders, asking where to save, what Esc does, recording options.

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

Then run `flick settings` and press **Load in Hyprland**: Flick writes its keybindings to `~/.config/hypr/flick.lua` (or `flick.conf`) and adds one line to your config that loads it. Remove other bindings on the same keys, such as an old `Print` screenshot bind; the settings window warns about them.

## Usage

| Command | Default key | Does |
|---|---|---|
| `flick` | Print | Main behavior after selecting a region |
| `flick --alt` | Alt+Print | Alternative behavior |
| `flick fastshot` | Shift+Print | Copies and/or saves the locked region at once, with no UI; without a lock you select a region first |
| `flick settings` | | Opens the settings |

Behaviors that do not show the toolbar act as soon as the mouse is released; with a locked region they act at once. Behaviors that show the toolbar can copy or save right after selecting too; later changes and annotations update the same file.

| Button | Action |
|---|---|
| Settings | Open the settings; the overlay comes back when they close |
| Lock | Pin or unpin the region |
| Play | Start recording; press again to stop and preview |
| Edit | Open the editor |
| Copy | Copy the image and close |
| Download | Save the image and close |
| Close | Close, copying or saving first if the settings say so |

| Key | Action |
|---|---|
| Esc | Same as Close; in the editor, finish the text or leave the editor |
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
flick record --region 0,0,400,300 --seconds 5 --file a.mp4   # --mic, --no-audio
```

## Configuration

The settings window writes `~/.config/flick/config.toml`; every field is optional:

```toml
file_name = "Flick_%Y-%m-%d_%H-%M-%S"  # strftime pattern, no extension
fps = 30
audio = true             # system audio
microphone = false       # default input device, mixed with system audio
cursor = true            # pointer in videos
confirm_discard = true   # ask before closing an unsaved recording
guides = true            # dashed guide lines while selecting

[quit]                   # what Esc and Close do first
copy = false
save = false

[main]
key = "Print"            # Hyprland notation; "" unbinds
copy = true
save = true
open = false             # show the toolbar

[alternative]
key = "ALT + Print"
copy = true
save = true
open = true

[fastshot]
key = "SHIFT + Print"
copy = true
save = true
ask = false              # ask where to save every fastshot

[screenshots]
dir = "/home/me/Pictures/Flick"  # default: Flick in your pictures directory
ask = false                      # Download asks where to save

[recordings]
dir = "/home/me/Videos/Flick"    # default: Flick in your videos directory
ask = false                      # every save asks where
```

The main and the alternative behavior must differ, one of them must show the toolbar, and the keys must differ. Videos are saved as MP4; copying a recording saves it first, because the clipboard holds the file.

## Limitations

- Requires a compositor with `wlr-layer-shell` and `wlr-screencopy`; tested only on Hyprland. X11, GNOME and KDE are not supported.
- No hardware encoding (video is encoded with x264).
- Multiple monitors and fractional scaling are not fully tested yet.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Architecture and UI testing are described in [CLAUDE.md](CLAUDE.md).
