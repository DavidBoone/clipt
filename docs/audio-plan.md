# Audio in recordings: implementation plan

Status: proposed, 2026-11-01. Nothing built yet.

## Goal

`clipt record --audio` adds sound to a recording: what the computer is
playing, the microphone, or both. Audio stays in sync with the video over
long recordings and across pauses, costs little CPU, and is off unless asked
for, so nobody's microphone is recorded by surprise.

## Decisions

| Question | Decision | Why |
|---|---|---|
| Capturing sound | libpulse-simple, loaded at run time (`dlopen`) like libpipewire | One small API (`pa_simple_new`, `pa_simple_read`, `pa_simple_get_latency`, `pa_simple_free`) that works the same on Wayland, X11 and in the Flatpak. PipeWire desktops serve it through pipewire-pulse, which every major distribution installs; plain PulseAudio serves it natively. The library is in the Flatpak runtime and on this machine. Not linked, so clipt still starts without it |
| Not ffmpeg's own audio input (`-f pulse`) | Rejected | ffmpeg would stamp audio with its own clock, and keep recording through a pause. clipt's video timestamps are its own (see the recording plan's writer), so audio has to be too |
| Not native PipeWire audio | Rejected for now | clipt's PipeWire code handles video formats only, and the Flatpak can't reach PipeWire for audio without a permission Flathub discourages (`xdg-run/pipewire-0`); it gets audio through the PulseAudio socket. Revisit only if pipewire-pulse turns out to be missing somewhere that matters |
| Getting audio to ffmpeg | Extra tracks in the Matroska stream clipt already writes (`A_PCM/INT/LIT`, 16-bit, 48 kHz, stereo) | Audio and video timestamps come from one clock, in one stream, so pause and sync are handled in one place |
| Sources | `system` (the default output's monitor, `@DEFAULT_MONITOR@`), `mic` (`@DEFAULT_SOURCE@`), or `both` | Covers tutorials (`both`), demos (`system`) and narration (`mic`). Picking a specific device can come later |
| Both at once | Two audio tracks, mixed by ffmpeg (`amix`) | Each source keeps its own clock and latency; mixing in clipt would mean resampling one to match the other |
| Encoding | AAC (ffmpeg's built-in `aac`), 160 kb/s, in every container | MP4 and MOV need it, MKV takes it, and it plays everywhere. Opus is better per bit but not in MOV, and is awkward in MP4 for some players |
| GIF | No audio | GIF can't hold sound; `--audio` with `--gif` says so and records without it |
| Default | Off | Recording the microphone without being asked would be a privacy failure |

## How it fits together

```
 video source ──frames──▶ camera ──▶ writer ─┐
                                              ├─▶ Matroska on ffmpeg stdin ──▶ file.mp4
 audio source(s) ──chunks (+ capture time)──▶─┘    (track 1 video, 2–3 audio)
 (libpulse-simple, own thread each)
```

- **Audio source** (`src/record/audio.rs`): one thread per source, blocking
  in `pa_simple_read` on 20 ms chunks (1 920 bytes at 48 kHz, 16-bit
  stereo). Each chunk is stamped with when its first sample was heard: the
  monotonic time when the read returned, minus the chunk's length and the
  latency `pa_simple_get_latency` reports. Chunks go to the writer through a
  bounded queue. Audio can't be dropped the way video frames are without
  causing gaps, but a full queue means ffmpeg has stalled, so it ends the
  recording with an error rather than eating memory.
- **Writer** (`src/record/mod.rs`): already turns capture times into stream
  times, with paused time subtracted. Audio chunks go through the same
  function. Chunks captured while paused are thrown away. The stream's zero
  becomes the recording's start, not the first video frame, so audio that
  begins before the screen first changes isn't lost. Clusters are written in
  timestamp order: audio is held back until video has caught up, but never
  for longer than 200 ms, because video is sparse on a still screen.
- **Matroska** (`src/record/mkv.rs`): a second (and third) TrackEntry
  (`A_PCM/INT/LIT`, SamplingFrequency 48 000, Channels 2, BitDepth 16).
  SimpleBlocks already carry a track number, so `frame_prefix` gains a
  track argument.
- **Encoder** (`src/record/encode.rs`): maps the audio tracks and adds
  `-c:a aac -b:a 160k`. A resampling filter (`aresample=async=1`) takes up
  the slow drift between the sound card's clock and the system clock
  (typically tens of parts per million, a few tenths of a second per hour)
  and fills any tiny gap between chunks. With two sources:
  `amix=inputs=2:duration=longest:normalize=0`, so neither source is
  halved in volume.

## Keeping sound and picture in sync

Both are stamped on the monotonic clock at capture, so they line up by
construction. What can still go wrong, and the answer to each:

- **Latency guesses.** If `pa_simple_get_latency` is off, everything is
  shifted by a constant. Phase 0 measures this with a beep and a flash.
  If it's off, a fixed correction, and a `record_audio_offset_ms` setting
  as the escape hatch.
- **Clock drift** over long recordings: the resampler above, and an
  hour-long test in phase 0.
- **Still screens.** On a still screen ffmpeg gets audio and no video for a
  long time. Its muxer waits up to 10 s (`max_interleave_delta`) for the
  other stream before writing, which is harmless for a file. If phase 0 shows
  otherwise, the writer re-sends the last frame once a second while audio
  is on, which costs almost nothing.
- **Pause.** No sound is captured while paused, and audio after resuming
  is shifted back by the same paused time as the video, so the cut is in
  the same place in both.

## User interface

- `clipt record --audio[=system|mic|both]`; a bare `--audio` means
  `system`. `--no-audio` overrides the setting for one recording.
- Setting: `record_audio = "none"` (default), `"system"`, `"mic"` or
  `"both"`.
- Overlay in record mode: a toggle in the toolbar (speaker, microphone, off)
  that starts from the setting. It changes only that recording, not the
  setting.
- Tray: a *Record with sound* checkbox under the record entries.
- Errors are clear and stop before recording starts: no libpulse-simple, no
  sound server, or no microphone, each saying what to install or check.
  The recording doesn't quietly go ahead without sound.
- The man page, completions, README Usage and CHANGELOG get the new
  options.

## Flatpak

- `--socket=pulseaudio` (the usual permission for apps that play or record
  sound; recording the microphone through it is allowed on today's Flatpak).
- The manifest's ffmpeg gains what it needs and nothing more: the
  `pcm_s16le` decoder, the `aac` encoder, swresample (currently disabled),
  and the `aresample`, `amix` and `aformat` filters.

## Testing

- Unit tests: Matroska audio track entries, chunk timestamps (latency,
  pauses), interleaving order.
- e2e (Sway and GNOME suites first): a PulseAudio or pipewire-pulse server
  with a null sink in the container. Play a 1 kHz tone, record, and check
  the result with `ffprobe` and `astats`: there is an AAC track, the tone is
  in it, and its duration matches the video. Pause test: the tone plays only
  while paused, and the file must be silent there. Sync test: the test window
  flashes white and a beep plays at the same moment, and the beep's onset
  must land within 40 ms (about one frame) of the flash.
- Flatpak: the same tone test through `clipt-launch` against headless
  Sway, as done for video.
- By hand: 30 minutes with a video playing on Plasma, then the sync
  checked at the end; then once on GNOME.

## Phases

Each phase ends with something usable, tests in CI, and a PR.

**0. Spike.** Load libpulse-simple and read `@DEFAULT_MONITOR@` in the
Sway container. Measure reported against real latency (beep and flash)
and drift against the monotonic clock over an hour. Pipe a Matroska
stream with PCM and sparse video through ffmpeg and check the MP4's sync and
duration. Results recorded here, decisions revisited if needed.

**1. System sound.** `audio.rs`, Matroska audio track, writer
interleaving, AAC encoding, pause, `--audio` and `record_audio` (`none` and
`system`), errors. e2e tone and pause tests on Sway, GNOME and X11.

**2. Microphone and both.** `mic` and `both`, the `amix` path, the e2e sync
test, the microphone-missing error.

**3. Controls and packaging.** Overlay toggle, tray checkbox, Flatpak
permission and ffmpeg components, man page, completions, README, CHANGELOG.

## Risks

- **pipewire-pulse missing** on a minimal PipeWire install: clipt says so.
  Native PipeWire audio is the fallback to build only if this proves
  common.
- **Latency reporting varies** between PulseAudio and pipewire-pulse
  versions: phase 0 measures both, and there's the offset setting.
- **Bluetooth headsets** switch profile (and sample rate) when the
  microphone opens, which can change the system sound mid-recording. The
  sound server resamples to the 48 kHz clipt asks for, so the stream stays
  valid. Worth one manual test.
- **Echo with `both`**: through speakers, the microphone picks up the
  system sound as well. Not clipt's to solve; the README suggests headphones.

## Not in this plan

Choosing devices other than the defaults, per-source volume, noise
suppression, recording one application's sound only (PipeWire can, through
the portal, but not through the PulseAudio API), and webcam overlay.
