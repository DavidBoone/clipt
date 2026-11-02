# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/).

## [Unreleased]

## [0.5.0] - 2026-11-02

### Added
- The screenshot overlay can record a video instead: click **Video** in the
  Screenshot | Video switch at the bottom of the screen, the camera button
  in the toolbar, or press Ctrl+R; then select, toggle the computer's sound
  (S) and the microphone (M), and press Enter. It starts from the sound
  `record_audio` sets.

## [0.4.2] - 2026-11-02

### Fixed
- Pasting a screenshot into Chromium-based apps (Edge, Teams, Chrome)
  could give only the top of the image, grey below: those apps hand over a
  non-blocking pipe, and clipt stopped writing once its 64 KB buffer filled.

## [0.4.1] - 2026-11-02

### Added
- Window highlighting on GNOME, through an optional GNOME Shell extension
  (`clipt gnome-extension` installs it; it answers only clipt). GNOME
  doesn't tell applications where windows are, so without it the overlay
  still highlights whole monitors there.

## [0.4.0] - 2026-11-02

### Added
- GNOME screenshots come from Mutter's ScreenCast (`-b mutter`) instead of
  the screenshot portal: no white flash, no PNG written to ~/Pictures and
  read back, and each monitor at its own resolution rather than the whole
  desktop at the highest monitor scale, so the overlay opens several times
  sooner on multi-monitor desktops with mixed scaling. A monitor Mutter
  sends nothing for within half a second still comes from the portal.
- `clipt record --audio-balance=duck` (or `record_audio_balance = "duck"`)
  ducks the other sound under the microphone: the microphone's sound is
  cleaned up in the mix, and the other sources dip by about 11 dB while the
  voice speaks. Needs the microphone and at least one other source.
- `--audio-balance=smart` ducks too, and at the stop says how clearly the
  voice came through ("speech clarity 0.84, good"). If it isn't good and
  turning the other sound down helps, it mixes the sound again with the
  other sources as much quieter as needed (up to 20 dB; the video is
  copied, not re-encoded). If the microphone
  hears the speakers, it says how far the voice is over the rest instead.

### Fixed
- A recording no longer hangs for ever if ffmpeg stalls at the end: after
  30 s without it finishing, clipt kills it and reports the partial file.
  Making a GIF gives ffmpeg 60 s plus twice the recording's length, then
  keeps the MP4 and says the GIF failed.

## [0.3.0] - 2026-11-02

### Added
- `clipt record --audio` records the sound the computer is playing along
  with the video, kept in sync and left out while paused. Off unless asked
  for (`--audio`, or `record_audio = "system"`); `--no-audio` overrides the
  setting. Needs a PulseAudio or pipewire-pulse server: libpulse-simple is
  loaded only when recording sound, so clipt still starts without it. On
  plain PulseAudio (not PipeWire) the first second or so of sound is
  silent: PulseAudio is slow to deliver the first chunk.
- Several sound sources at once, each with its own volume:
  `--audio=system:70,mic:120`, `--audio=mic`, `--audio=both`, or devices
  by name. They're mixed into one track; `--separate-audio` (or
  `record_separate_audio`) also keeps each source as its own track, at the
  volume it was recorded, to rebalance later. `clipt list sound` lists the
  devices and marks what `system` and `mic` stand for.
- Sound controls: in `clipt record`'s overlay, speaker and microphone
  buttons (or **S** and **M**) turn the computer's sound and the microphone
  on or off for that recording; the tray menu has a *Record sound*
  checkbox.
- The Flatpak records sound too.

### Changed
- Window recordings that zoom scale each frame 25–30 % faster.

### Fixed
- The man page and the shell completions were missing `--gif` and
  `record_format`, and the man page and README still said X11 and the
  Flatpak couldn't record.

## [0.2.0] - 2026-11-01

### Added
- `clipt record`: record the screen, a region or a window to MP4, MKV, MOV
  or GIF. With no arguments it opens the selection overlay in record mode
  (Enter records, click a window to record it); `clipt record screen
  [NAME]`, `-g GEOM` and `window APP` skip the overlay. Stop with
  `clipt record --stop`, Ctrl-C or the tray icon; `--pause`, `--resume`
  and `--toggle-pause` leave paused time out of the video.
- Recording works on wlroots compositors (Sway, Hyprland, niri, labwc,
  COSMIC, …), KDE Plasma, GNOME and X11, and through the ScreenCast portal
  elsewhere (its dialog appears once; the choice is remembered). On KDE and
  GNOME frames arrive over PipeWire, whose library is loaded only when
  recording, so clipt still starts without it.
- Windows are recorded on their own where the compositor can (KDE Plasma,
  Sway 1.11+, Hyprland 0.50+, COSMIC, or picked in the portal's dialog) and
  followed by position on older Sway and Hyprland, and on X11. When a
  recorded window resizes, the video keeps its size and the zoom eases to
  fit.
- Encoding runs ffmpeg (needed only for recording) on the GPU where
  possible (VA-API, NVENC), libx264 otherwise. Frames are only captured and
  encoded when the screen changes, so a still screen costs almost nothing.
  GIFs (`--gif`, or an output name ending in `.gif`) are made when the
  recording stops, at up to 15 fps.
- On wlroots compositors a red frame outside the area marks a region
  recording, amber while paused. The tray gains *Record region…*, *Record
  screen*, *Pause/Resume/Stop recording* and *Open recordings folder*, and
  its icon shows a red (paused: amber) dot while recording.
- New config keys: `video_dir` (default `~/Videos/Screencasts`),
  `video_filename`, `record_fps`, `record_codec`, `record_format` and
  `max_duration` (default 1 hour).
- A man page (`man clipt`) and bash, zsh and fish completions, installed by
  the packages, `install.sh` and the release tarballs.
- The Flatpak (still a prototype, built from `packaging/flatpak`) records
  too, with its own ffmpeg.

## [0.1.1] - 2026-10-31

### Fixed
- clipt hung at startup, before showing anything, when xdg-desktop-portal had
  stopped answering, even on KDE where the portal isn't used to capture. D-Bus
  calls now give up after 5 seconds and clipt carries on without that backend.
- A faint one-pixel seam could flash along the window highlight while it
  glided between windows; the highlight now moves in whole pixels.

## [0.1.0] - 2026-10-31

First public release.

### Added
- `clipt tray`: system tray icon (StatusNotifierItem + dbusmenu). Click for a
  region, middle-click for the full desktop, menu for delayed capture, the
  screenshots folder and Quit. `install.sh --tray` adds it to autostart.
- clipt icon (flat mascot holding a picture frame): drawn in code for the tray,
  simplified and pixel-snapped below 32 px; `data/clipt.svg` for the desktop entry.
- Window selection: before selecting, the window under the pointer (or the
  screen, over bare desktop) is highlighted with its name and size; a click
  takes it, Tab switches window/screen. Window positions come from KWin, Sway,
  Hyprland or EWMH on X11, fetched while the screen is captured.
- HUD-style hint bar with key caps; the hover highlight eases between targets.
- `clipt update [--check]` and *Check for updates…* in the tray: installs the
  new release in place for install.sh installs (SHA-256 verified), points
  packaged installs at their package manager. Opt-in daily check with
  `check_updates = true`.
- Release workflow: one click bumps, tags and publishes packages.
- Frozen-screen region selection with resize handles, magnifier and
  click-to-select-a-monitor; selections can span monitors.
- Annotation: rectangle, ellipse, arrow, line, pen, highlighter, text,
  pixelate and numbered counters; 8 colours, adjustable width, undo/redo.
- Copy (clipt stays in the background to serve the clipboard, ~5 MB), save,
  desktop notifications.
- Capture backends picked automatically: ext-image-copy-capture,
  wlr-screencopy, KWin ScreenShot2 and xdg-desktop-portal.
- Overlay on every monitor at once at native resolution: no monitor
  picker, correct with mixed resolutions, fractional scaling and rotated
  monitors.
- X11 support (Xfce, Cinnamon, MATE, LXQt, i3, Openbox, …): RandR or
  Xinerama monitors, MIT-SHM overlay, CLIPBOARD with INCR for large images.
- Builds on glibc and musl Linux (x86_64, aarch64, armv7, riscv64) and FreeBSD.
- `full`, `screen` and slurp-compatible `-g` modes; stdout output.
- End-to-end test suites on headless Sway, GNOME Shell and X11.

### Fixed
- Redraws while moving the pointer repainted the whole screen: the crosshair's
  two lines were merged into one screen-sized damage box. Now ~4 ms median on
  4K instead of 7-12 ms.
- The tray kept launching the old binary path after an upgrade replaced it.
