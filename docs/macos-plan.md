# macOS: plan and status

Status, 2026-11-03: phase 1 (screenshots) shipped in 0.6.0 and has run on
one real Mac, where the overlay's lag was found and fixed. Treat it as a
preview until a tester has worked through the checklist below.

## Goal

The same clipt on macOS 12 and later, on Apple silicon and Intel: select a
region, a window or a display, annotate, copy or save; later, record. One
binary, nothing to install beside it except ffmpeg for recording.

## How it fits

The editor (`src/editor/`), images, text, the config and PNG output are
shared. macOS gets its own host in `src/macos/`, beside the Wayland and
X11 ones, and the Linux-only code and crates are behind
`cfg(not(target_os = "macos"))`.

| Part | Linux | macOS |
|---|---|---|
| Displays | Wayland outputs, RandR | `CGGetActiveDisplayList`, `CGDisplayBounds`, names from `NSScreen` |
| Screenshots | `capture/` | `CGDisplayCreateImage`, drawn into sRGB; `screencapture` as the fallback and for `--cursor` |
| Showing the editor | `overlay.rs`, `x11/overlay.rs` | `macos/overlay.rs`: a borderless window per display at screen-saver level, a layer showing the editor's bitmap |
| Window picker | `windows/linux.rs` | `CGWindowListCopyWindowInfo` (layer 0, frontmost first) |
| Clipboard | Wayland and X11 selections, which clipt serves | `NSPasteboard` (PNG and TIFF); the system keeps it, so clipt exits at once |
| Fonts | `fc-match` | Arial from `/System/Library/Fonts/Supplemental` |
| Notifications | D-Bus | `osascript` (`display notification`) |
| Shortcuts | ⌃C ⌃S ⌃Z | ⌘C ⌘S ⌘Z (⌃ works too) |

Decisions:

| Question | Decision | Why |
|---|---|---|
| Bindings | `objc2` and its framework crates | Safe, maintained bindings to frameworks every Mac has, so the no-new-C-libraries rule holds |
| Capture | Core Graphics, not ScreenCaptureKit | Synchronous, no run loop or async completion handlers, and on every macOS clipt targets. It's deprecated, so it's looked up at run time (`dlsym`), and `screencapture` takes over if it ever goes. Recording will need ScreenCaptureKit anyway |
| Event loop | clipt's own `nextEventMatchingMask` loop | `run` returns when the user decides, like the other hosts |
| Permission | Checked first (`CGPreflightScreenCaptureAccess`), with the system prompt on the first run | Without it macOS returns screenshots with every window blank, which looks like a bug |

## Phases

**1. Screenshots (this one).** `clipt`, `clipt full`, `clipt screen`,
`clipt list`, copy, save, notifications, window highlighting. CI builds a
universal binary (`clipt-macos` artifact on each run of the `macos` job).

**2. Menu bar icon and a hotkey.** `clipt tray` (`src/macos/tray.rs`), and
what opening clipt.app with no arguments does: an `NSStatusItem` with the
Linux tray's screenshot actions, Open at Login (`SMAppService`, macOS 13+,
from the app only), and global shortcuts through `RegisterEventHotKey`,
which needs no Accessibility permission: `hotkey` (a region, default
`cmd+shift+2`) and `hotkey_full` (the whole desktop, off) in the config.
Opening the app again while it runs captures a region. Each action starts a
fresh `clipt`, as on Linux; one that fails says why in a notification.

**3. Recording.** `src/macos/record.rs`: ScreenCaptureKit (`SCStream`,
macOS 12.3+) for frames, handed to the same writer as Linux through
`record::Capture` (the Linux sources are in `src/record/linux.rs`). A
display is cropped by `sourceRect`, at the display's pixel scale; a window
is captured on its own (`desktopIndependentWindow`), so it records while
covered, at the size it had when recording started. ffmpeg's
`h264_videotoolbox` encoder (its constant-quality mode needs Apple
silicon; Intel Macs fall back to libx264), found on PATH or where Homebrew
and MacPorts put it, since an app opened from Finder has only the system's
PATH. `--stop` and `--pause` use the same pid file and FIFO as Linux, in
`$TMPDIR`. The menu bar icon has Record Region, Record Screen, and while
recording, Pause and Stop, and turns into a red record symbol. Not yet:
a frame around the area being recorded (the main thread is busy writing,
and AppKit draws only there), and sound, whose toggles the overlay hides.

**Whole windows in screenshots.** The overlay shows the screen as it was
when it opened, so a covered window would be saved covered. Clicking a
window (`src/macos/window_shot.rs`) captures it on its own and paints it
over that picture (`Editor::paste`), as if brought to the front: with
ScreenCaptureKit's `SCScreenshotManager` from macOS 14, and before that
`CGWindowListCreateImage`, looked up at run time.

**4. Sound.** `src/macos/sound.rs`, behind the same `check`, `open` and
`Capture::run` as PulseAudio's (`src/record/audio/pulse.rs`). `system` is
ScreenCaptureKit's audio (macOS 13+) on a stream of its own, leaving
clipt's own sound out; `mic` is the default input through an Audio Queue,
which converts to 48 kHz 16-bit stereo itself (`NSMicrophoneUsageDescription`
in Info.plist; macOS asks once). Buffers are stamped from the host clock,
which `Instant` runs on. Silence fills in when ScreenCaptureKit sends
nothing. Not yet: devices by name, and `clipt list sound`.

**App bundle.** `packaging/macos/build-app.sh` wraps the binary in
`clipt.app` (bundle ID `io.github.jasondyck.clipt`, no Dock icon, the icon
drawn by `clipt debug-icon -o clipt.iconset` and packed by `iconutil`). macOS then files the Screen
Recording permission under clipt itself, not the terminal or launcher that
ran it, and the app can live in Applications and Login Items. CI uploads it
as `clipt-app.zip` inside the `clipt-macos` artifact; releases carry
`clipt-vX.Y.Z-macos-app.zip`, notarised with the bare binary in one
submission and stapled. Opened from Finder it starts the editor; once the
menu bar icon exists it will start that instead. Keep the bundle ID: a
new one makes macOS ask every user for the permission again.

**5. Release.** Done ahead of the others: the release workflow adds a
signed and notarised universal tarball (`clipt-vX.Y.Z-universal-macos.tar.gz`)
to each GitHub release, and `clipt update` installs it.

## Signing

The `macos` CI job signs the binary with the project's Developer ID
(hardened runtime, secure timestamp) and has Apple notarise it, so
Gatekeeper lets a downloaded copy run. It needs five repository secrets:
`MACOS_CERT_P12` (the Developer ID Application certificate and its key, as
base64), `MACOS_CERT_PASSWORD`, and the App Store Connect API key used for
notarisation, `MACOS_NOTARY_KEY` (the `.p8` file's text),
`MACOS_NOTARY_KEY_ID` and `MACOS_NOTARY_ISSUER`. Without them (pull
requests from forks), the build is ad-hoc signed and a downloaded copy
needs `xattr -d com.apple.quarantine clipt` before it runs. A bare binary
can't carry its notarisation ticket, so the first run checks with Apple
online.

The job waits at most ten minutes for Apple, then uploads the signed build
with a warning naming the submission. To see where a submission stands,
run the `notary` workflow (Actions → notary → Run workflow), with or
without a submission id.

## Testing

CI can't show a screen to anyone, so phase 1 needs a person with a Mac:

1. Download `clipt-macos` from the latest run of the `macos` CI job, unzip
   it, and `chmod +x clipt`. (An ad-hoc signed build, from a run without
   the signing secrets, also needs `xattr -d com.apple.quarantine clipt`.)
2. `./clipt --version` and `./clipt list`: each display, with its size.
3. `./clipt full -o test.png`: the first time, macOS asks to allow Screen
   Recording for the terminal; allow it, quit and reopen the terminal, run
   it again. The PNG should show every window, at full Retina resolution.
4. `./clipt`: the screen freezes on every display. Check that the pointer
   highlights windows, a drag selects, the toolbar's tools draw, ⌘Z undoes,
   Return copies (paste into Notes, Mail, Messages, Slack), ⌘S saves to
   `~/Pictures/Screenshots`, Esc cancels.
5. Keys work without clicking first. If they only work after a click,
   that's macOS refusing clipt focus, and worth reporting.
6. With two displays, especially a Retina one next to a non-Retina one:
   selections on each and across both.
7. A full-screen app on its own Space: the overlay should cover it.

Recording (phase 3), with ffmpeg installed (`brew install ffmpeg`):

1. `./clipt record screen`, wait a few seconds, Ctrl-C: an MP4 in
   `~/Videos/Screencasts` (`video_dir`), at full Retina resolution, smooth.
   `CLIPT_DEBUG=1` prints the encoder and frames captured.
2. `./clipt record`: the overlay, with no sound toggles. Drag an area,
   Enter; stop with `./clipt record --stop` from another terminal. Only the
   area is in the video.
3. `./clipt record`, click a window, Enter, then cover it with another
   window: the video shows the window, not what covered it.
4. `--pause` and `--resume` mid-recording: the paused part is left out.
5. `--gif`, `-g "100,100 640x480"`, `--cursor`.
6. In clipt.app's menu: Record Region, then the icon turns red and the menu
   has Pause and Stop; Stop saves and notifies.
7. Without ffmpeg (or with it renamed): Record Screen from the menu shows a
   notification saying to install it.

Report what you see in the PR or an issue, with `CLIPT_DEBUG=1` output for
anything odd.
