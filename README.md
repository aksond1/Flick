# Flick

Region screenshots and screen recording for Hyprland (Wayland), written in Rust with GTK4.

Everything happens inside the selected region: selecting, editing, recording and playback. Only the settings have a window of their own.

> Work in progress. Screenshots, recording, preview, the editor, the settings and multiple monitors are ready.

## Features

- **Selection:** every monitor freezes and you select on any of them; move the region, resize it with 8 handles or their dashed guide lines (which can be turned off), or scale it with the mouse wheel. The region stays within one monitor.
- **Lock:** pins the region, so the next launch opens with it right away; a pinned region can still be moved. It is forgotten when its monitor changes resolution or scale.
- **Screenshots:** copy to the clipboard or save as PNG.
- **Recording:** MP4 with system audio and, optionally, the microphone; pause and resume as often as you like, and move the region while recording.
- **Preview:** the video plays inside the region, with seeking and volume; save it, or copy it to the clipboard without saving.
- **Editor:** brush and text, any color, font, size, undo, redo, clear.
- **Two keybindings:** a main and an alternative behavior after selecting (copy, save, show the toolbar, in any useful mix). With a locked region, a behavior that does not show the toolbar shoots at once, with no UI.
- **Settings window:** keybindings written into your Hyprland config, folders, asking where to save, file names, what Esc does, guide lines, frame rate, audio, microphone, pointer.

## Installation

### Arch Linux package

```sh
git clone https://github.com/aksond1/Flick.git
cd Flick/packaging/arch
makepkg -si
```

This builds the latest commit from GitHub and installs `flick`, plus a **Flick Settings** entry for app launchers.

### From source

Dependencies (Arch Linux):

```sh
sudo pacman -S --needed gtk4 gtk4-layer-shell gst-plugin-gtk4 gst-plugins-base \
  gst-plugins-good gst-libav ffmpeg wl-clipboard pipewire-pulse
```

Building also requires Rust (edition 2024).

```sh
cargo build --release
install -Dm755 target/release/flick ~/.local/bin/flick   # any directory in PATH
```

Then run `flick settings` and press **Load in Hyprland**: Flick writes its keybindings to `~/.config/hypr/flick.lua` (or `flick.conf`) and adds one line to your config that loads it. From then on the settings keep that file up to date and reload Hyprland themselves. Remove other bindings on the same keys, such as an old `Print` screenshot bind; the settings window warns about them.

When `flick` is not in `PATH`, the bindings use the full path of the binary that wrote them, so run `flick settings` again after moving it.

## Usage

| Command | Default key | Does |
|---|---|---|
| `flick` | Print | Main behavior after selecting a region |
| `flick --alt` | Alt+Print | Alternative behavior |
| `flick settings` | | Opens the settings |

Behaviors that do not show the toolbar act as soon as the mouse is released; with a locked region they act at once. Behaviors that show the toolbar can copy or save right after selecting too; later changes and annotations update the same file.

| Button | Action |
|---|---|
| Settings | Open the settings; the overlay comes back when they close |
| Lock | Pin or unpin the region; a pinned region can still be moved by dragging it, and stays pinned there |
| Play | Start recording; press again to stop and preview |
| Pause | While recording: pause, then resume; the paused time is left out of the video and the timer |
| Edit | Open the editor |
| Copy | Copy the image and close |
| Download | Save the image and close |
| Close | Close, copying or saving first if the settings say so |

| Key | Action |
|---|---|
| Esc | Same as Close; in the editor, finish the text or leave the editor |
| Space | Pause or play during preview |
| Mouse wheel | While selecting: scale the region around its center, up to enlarge (not a locked region, not in the editor) |
| Ctrl+Z | Undo (editor) |
| Ctrl+Y, Ctrl+Shift+Z | Redo (editor) |

- Launching `flick` again while recording stops it. This is the only way when the region fills the screen and there is no room for the toolbar.
- While recording, move the region with the dotted grip on the toolbar; it cannot be resized.
- Starting a recording closes the overlays on the other monitors, so they show the live screen.
- In the preview, Copy puts the file on the clipboard without saving it to your videos folder (Flick keeps the latest copied recording in `~/.cache/flick/clipboard`). Save keeps it in the folder.
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

[screenshots]
dir = "/home/me/Pictures/Flick"  # default: Flick in your pictures directory
ask = false                      # Download asks where to save

[recordings]
dir = "/home/me/Videos/Flick"    # default: Flick in your videos directory
ask = false                      # every save asks where
```

The main and the alternative behavior must differ, one of them must show the toolbar, and the keys must differ. Videos are saved as MP4.

## Limitations

- Requires a compositor with `wlr-layer-shell` and `wlr-screencopy`; tested only on Hyprland. X11, GNOME and KDE are not supported.
- No hardware encoding (video is encoded with x264).
- A region cannot span two monitors.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Architecture and UI testing are described in [CLAUDE.md](CLAUDE.md).

## License

Flick is free software under the [GNU General Public License v3.0 or later](LICENSE): you may use, study, change and share it, and versions you distribute must stay open under the same license.
