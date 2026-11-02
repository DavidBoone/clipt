# Audio in recordings: implementation plan

Status: phases 0–2 done (system sound; microphone, several sources, volumes,
separate tracks, devices), 2026-11-01. Phase 3 (controls, Flatpak) next.

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
| Sources | Up to 8: `system` (the default output's monitor, `@DEFAULT_MONITOR@`), `mic` (`@DEFAULT_SOURCE@`), or devices by name; `both` means `system,mic` | Covers tutorials (`both`), demos (`system`) and narration (`mic`); names cover a second microphone or another output. Added in phase 2 on request, with `clipt list sound` to find the names |
| Several at once | One audio track per source, each through its own `volume`, mixed by ffmpeg (`amix`, `normalize=0`) | Each source keeps its own clock and latency; mixing in clipt would mean resampling one to match the other. `normalize=0` keeps the volumes asked for, rather than dividing by the number of sources |
| Balance | A volume per source, 0–400 %, set before recording (`system:70,mic:120`); optionally each source also kept as its own track at its recorded level (`--separate-audio`), after the mix | Fixed volumes give one track that plays right everywhere; separate tracks let the balance be fixed afterwards in an editor. Changing volumes during a recording is left for later (see *Not in this plan*) |
| Listing devices | libpulse's asynchronous API, loaded like libpulse-simple, reading only the first fields of `pa_server_info` and `pa_source_info` | libpulse-simple can't list anything; the first fields of those records haven't changed in libpulse's history, and a unit test pins their offsets |
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
  in `pa_simple_read` on 20 ms chunks (3 840 bytes at 48 kHz, 16-bit
  stereo; `fragsize` set to match, or the server hands over much larger
  fragments). Chunks are stamped by a sample clock: an anchor plus the
  samples read so far, divided by 48 000. Each read gives one estimate of the
  anchor: when the read returned, minus the reported latency and the
  samples read. Reads can only return late, never early, so the anchor is
  the earliest estimate over the last 10 s (the window lets it follow clock
  drift). Stamping each chunk from its own read time instead jitters by
  ±15 ms (see the spike results). Chunks go to the writer through a
  bounded queue. Audio can't be dropped the way video frames are without
  causing gaps, but a full queue means ffmpeg has stalled, so it ends the
  recording with an error rather than eating memory.
- **Writer** (`src/record/mod.rs`): already turns capture times into stream
  times, with paused time subtracted. Audio chunks go through the same
  function, placed against a list of every pause so a chunk next to a pause
  lands exactly; chunks heard while paused are thrown away. Time zero stays
  the first video frame, which arrives at once (capture always sends a first
  frame), so at most a chunk is lost before it. Chunks are written as they
  arrive rather than interleaved by timestamp: ffmpeg orders the two tracks
  itself, and the tests show the lengths and pauses come out right. After a
  stop, the sound thread keeps reading until a chunk stamped after the stop
  arrives (at most 1 s), because the server's latency worth of sound is
  still in its buffer when the stop comes.
- **Matroska** (`src/record/mkv.rs`): a second (and third) TrackEntry
  (`A_PCM/INT/LIT`, SamplingFrequency 48 000, Channels 2, BitDepth 16).
  SimpleBlocks already carry a track number, so `frame_prefix` became
  `block_prefix`, with a track argument.
- **Encoder** (`src/record/encode.rs`): maps the audio tracks and adds
  `-c:a aac -b:a 160k`. A resampling filter (`aresample=async=1:first_pts=0`)
  takes up the slow drift between the sound card's clock and the system
  clock (under 2 parts per million in the spike, but other hardware can do
  worse), fills any tiny gap between chunks, and fills the wait before the
  first chunk with silence so the sound starts with the video (see
  *PulseAudio's slow start* under Risks). With two sources:
  `amix=inputs=2:duration=longest:normalize=0`, so neither source is
  halved in volume.

## Keeping sound and picture in sync

Both are stamped on the monotonic clock at capture, so they line up by
construction. What can still go wrong, and the answer to each:

- **Latency guesses.** If `pa_simple_get_latency` is off, everything is
  shifted by a constant. In the spike it was 3–6 ms early once settled, too
  small to correct. A `record_audio_offset_ms` setting stays as the escape
  hatch for setups that measure worse.
- **Clock drift** over long recordings: under 2 parts per million in the
  spike (about 7 ms an hour), and the resampler above for hardware that
  drifts more.
- **Still screens.** On a still screen ffmpeg gets audio and no video for a
  long time. Its muxer holds the audio until video catches up (in the spike
  an MP4 stayed at 48 bytes through 14.5 s of still screen), and the result
  is still in sync. That only matters if ffmpeg is killed: a crash loses
  that much audio, and an MP4 without its index at the end is unplayable
  anyway. So the writer doesn't need to re-send frames.
- **Pause.** No sound is captured while paused, and audio after resuming
  is shifted back by the same paused time as the video, so the cut is in
  the same place in both.

## User interface

- `clipt record --audio[=SOURCES]`: `system`, `mic`, `both`, or a comma-
  separated list of sources (`system`, `mic`, device names), each with an
  optional `:PERCENT` volume. A bare `--audio` means `system`.
  `--separate-audio` also keeps each source as its own track. `--no-audio`
  overrides the setting for one recording.
- Settings: `record_audio`, taking the same values (default `"none"`), and
  `record_separate_audio` (default `false`).
- `clipt list sound` lists the devices, marking what `system` and `mic`
  stand for. (Not new lines in `clipt list`: the Flatpak probe script counts
  its lines as outputs.)
- Track names: "Mix", then "System sound", "Microphone" or the device name;
  stored as the handler name in MP4 and MOV (what players show) and as the
  title in MKV.
- Overlay in record mode: a toggle in the toolbar (speaker, microphone, off)
  that starts from the setting. It changes only that recording, not the
  setting.
- Tray: a *Record with sound* checkbox under the record entries.
- Errors are clear and stop before recording starts: no libpulse-simple, no
  sound server, an unknown device (pointing at `clipt list sound`), or a
  volume out of range, each saying what to install or check. The recording
  doesn't quietly go ahead without sound. (A missing microphone can't be
  told apart: the server always has a default input, falling back to a
  monitor or a dummy device.)
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
  while paused, and the file must be silent there. Several sources: a second
  null sink stands in for the microphone with a 440 Hz tone, so each track
  can be checked by frequency. (The planned beep-and-flash sync test was
  dropped: the test's own playback latency, ffmpeg's PulseAudio output and
  PulseAudio's slow start, is unknown, so it would mostly measure itself.
  The spike measured stamping against a known reference instead.)
- Flatpak: the same tone test through `clipt-launch` against headless
  Sway, as done for video.
- By hand: 30 minutes with a video playing on Plasma, then the sync
  checked at the end; then once on GNOME.

## Phases

Each phase ends with something usable, tests in CI, and a PR.

**0. Spike (done 2026-11-01).** libpulse-simple through Python's ctypes,
and a Python Matroska writer copying clipt's. See the spike results below.

**1. System sound (done 2026-11-01).** `audio.rs`, Matroska audio track,
the writer's sound handling, AAC encoding, pause, `--audio`, `--no-audio`
and `record_audio` (`none` and `system`), errors (no libpulse-simple, no
sound server, an ffmpeg without AAC, `mic`/`both` not yet). The man page,
completions and README got the options now rather than in phase 3. Shared
e2e tests (`tests/audio.sh`) on Sway and X11 with PulseAudio, on GNOME
with pipewire-pulse: a tone is recorded at the right level, sound and video
last as long, a tone played only while paused is left out, `--gif` with
`--audio` warns, and a missing server stops the recording before it starts.

**2. Several sources (done 2026-11-01).** Widened on request from "`mic`
and `both`" to: any number of sources up to 8, a volume for each,
`--separate-audio`, devices by name and `clipt list sound`. e2e tests on
all three desktops: the listing, three tracks with the right names, each
separate track holding only its own tone (31 dB apart), the mix holding
both equally, `system:100,mic:25` putting the microphone 12 dB under,
a device by name, and the unknown-device and volume errors. Found on the
way: pipewire-pulse, unlike PulseAudio, accepts a stream from a device that
doesn't exist, and the recording ran on with nothing from it, so named
devices are now checked against the server's list before recording, and a
source that has delivered nothing after 5 s stops the recording.

**3. Controls and packaging.** Overlay toggle, tray checkbox, Flatpak
permission and ffmpeg components (now also `asplit`, `amix` and `volume`).
The man page, completions, README and CHANGELOG already cover phases 1
and 2.

## Spike results (2026-11-01)

Python scripts, not clipt code, so the numbers are about the approach.
PipeWire 1.6 with pipewire-pulse throughout.

**Timestamps.** A null sink in a container. The test played 1 kHz beeps at
moments known from the playback stream's own sample clock, recorded its
monitor, and compared each beep's recorded start with its played start,
15 beeps per run, three runs:

| Stamping | Recorded minus played | Spread |
|---|---|---|
| Each chunk from its own read time, minus latency | −3.3 to −4.7 ms mean | 28–32 ms, the same sawtooth every run |
| Sample clock, earliest anchor | −3 to −6 ms once settled (after ~4 s); up to +11 ms before | under 0.5 ms once settled |

The sawtooth is probably PipeWire processing in 1 024-sample (21.3 ms)
blocks against 20 ms reads, so reads return in bursts. The sample clock
removes it. The first seconds' error is the anchor still settling; it is
under one frame at 60 fps, and the writer already holds audio back for
200 ms, so stamps could be fixed up before sending if it ever matters.

**ffmpeg.** A Matroska stream from the spike writer: black frames at 0 and
0.5 s, a white frame at 15 s, a black one at 20 s, and continuous PCM with
beeps at 1 s and 15 s. Encoded with clipt's libx264 arguments plus
`-c:a aac -b:a 160k -af aresample=async=1`:

- MP4, MOV and MKV: the beeps at 1.000 and 15.000 s (±0.3 ms), and the
  flash at 15.000 s. ffmpeg handles AAC's start-up delay itself.
- Written in real time, with 14.5 s of still screen: the same result. The
  MP4 stayed at 48 bytes during the still screen (see *Still screens*).
- ffmpeg warns `Guessed Channel Layout: stereo`, which is right. Matroska
  has no channel-layout element; clipt's `-loglevel error` hides it.

**Drift.** libpulse-simple on a desktop with a Focusrite Scarlett Solo
(USB), counting samples from `@DEFAULT_MONITOR@` against the monotonic
clock for 10 minutes, with a silent stream playing so the sound card was
running and timing the graph. The difference never passed ±1.1 ms and
didn't build up: under 2 parts per million, so at most about 7 ms over an
hour. The resampler's drift correction is a safety net, not a necessity,
on hardware like this. A second run, where the card may have been idle,
agreed (±1.5 ms over 4 minutes). Other sound cards, and Bluetooth, may
drift more.

## Risks

- **PulseAudio's slow start.** Found in phase 1: PulseAudio (17, null
  sink) took 1.5 s to deliver the first 20 ms chunk, then ran normally;
  pipewire-pulse delivered it in 16 ms. Asking for the server's default
  fragment size made the first read quick but the second one slow
  instead. So on PulseAudio the first second or so of a recording is
  silent (padded by the resampler) rather than missing. Whether real
  sound cards on PulseAudio do the same is unchecked; opening the stream
  before the overlay would hide it there.
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

Changing volumes during a recording (it would mean applying them in clipt
before ffmpeg, with a command such as `clipt record --volume mic=+10%`;
worth it once the overlay or tray has controls for it), noise suppression,
recording one application's sound only (PipeWire can, through the portal,
but not through the PulseAudio API), and webcam overlay.
