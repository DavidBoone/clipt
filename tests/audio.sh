# Sound in recordings: tests shared by the Sway, GNOME and X11 suites.
# Source this after defining check(), then call
#   audio_tests GEOM
# with a region (slurp format) that records on this desktop. Starts a sound
# server first: pipewire-pulse where PipeWire is installed (GNOME's image),
# PulseAudio otherwise, so both are tested. A null sink stands in for the
# sound card, and ffmpeg plays a 1 kHz tone into it.

sound_server() {
  if command -v pipewire-pulse >/dev/null; then
    pgrep -x pipewire >/dev/null || { pipewire >/tmp/pw.log 2>&1 & sleep 0.5; }
    pgrep -x wireplumber >/dev/null || { wireplumber >/tmp/wp.log 2>&1 & }
    pipewire-pulse >/tmp/pp.log 2>&1 &
  else
    pulseaudio --daemonize=no --exit-idle-time=-1 >/tmp/pa.log 2>&1 &
  fi
  for _ in $(seq 50); do pactl info >/dev/null 2>&1 && break; sleep 0.1; done
  pactl load-module module-null-sink sink_name=clipt >/dev/null
  pactl set-default-sink clipt
  pactl info | sed -n 's/^Server Name: /  sound server: /p'
}

tone_on()  { ffmpeg -nostdin -loglevel error -re -f lavfi -i sine=frequency=1000:sample_rate=48000 -ac 2 -f pulse -device clipt tone & TONE=$!; }
tone_off() { kill $TONE 2>/dev/null; wait $TONE 2>/dev/null; }
# Mean and peak level of a file's sound, in dB.
mean_db()  { ffmpeg -nostats -i "$1" -map 0:a -af volumedetect -f null - 2>&1 | sed -n 's/.*mean_volume: \(-*[0-9.]*\) dB/\1/p'; }
max_db()   { ffmpeg -nostats -i "$1" -map 0:a -af volumedetect -f null - 2>&1 | sed -n 's/.*max_volume: \(-*[0-9.]*\) dB/\1/p'; }
a_codec()  { ffprobe -v error -select_streams a -show_entries stream=codec_name -of csv=p=0 "$1" 2>/dev/null; }
s_dur()    { ffprobe -v error -select_streams "$2" -show_entries stream=duration -of csv=p=0 "$1" 2>/dev/null; }

audio_tests() {
  local g=$1
  echo "-- recording: sound"
  sound_server

  /clipt record -g "$g" -o /tmp/snd-none.mp4 2>/tmp/snd.err & local rp=$!
  sleep 1.5; /clipt record --stop; wait $rp
  check "no sound unless asked (streams: $(a_codec /tmp/snd-none.mp4))" '[ -s /tmp/snd-none.mp4 ] && [ -z "$(a_codec /tmp/snd-none.mp4)" ]'

  tone_on; sleep 0.5
  CLIPT_DEBUG=1 /clipt record -g "$g" --audio -o /tmp/snd.mp4 2>/tmp/snd.err & rp=$!
  sleep 3; /clipt record --stop; wait $rp; local rc=$?
  tone_off
  check "--audio records ($rc; $(grep -iE 'error|can.t|fail' /tmp/snd.err | head -1))" '[ $rc = 0 ]'
  check "the sound is AAC ($(a_codec /tmp/snd.mp4))" '[ "$(a_codec /tmp/snd.mp4)" = aac ]'
  local m=$(mean_db /tmp/snd.mp4)
  check "the tone is in it (mean ${m} dB)" 'awk "BEGIN{exit !($m > -30)}"'
  local va=$(s_dur /tmp/snd.mp4 v) aa=$(s_dur /tmp/snd.mp4 a)
  check "sound and video last as long ($aa s, $va s)" 'awk "BEGIN{d=$aa-$va; exit !(d < 0.25 && d > -0.25)}"'
  grep -E "sound chunks" /tmp/snd.err | sed 's/^clipt: /  /'

  # The tone plays only while paused: none of it may be in the recording.
  /clipt record -g "$g" --audio -o /tmp/snd-p.mp4 2>/tmp/snd.err & rp=$!
  sleep 1; /clipt record --pause; sleep 0.3
  tone_on; sleep 1.5; tone_off; sleep 0.5
  /clipt record --resume; sleep 1; /clipt record --stop; wait $rp
  m=$(max_db /tmp/snd-p.mp4)
  check "sound while paused is left out (peak ${m} dB)" 'awk "BEGIN{exit !($m < -40)}"'
  va=$(s_dur /tmp/snd-p.mp4 v); aa=$(s_dur /tmp/snd-p.mp4 a)
  check "so is its time ($aa s of sound, $va s of video, ~2 s recorded)" 'awk "BEGIN{exit !($aa < 2.6 && $va < 2.6 && $aa-$va < 0.25 && $va-$aa < 0.25)}"'

  /clipt record -g "$g" --audio --gif -o /tmp/snd.gif 2>/tmp/snd.err & rp=$!
  sleep 1; /clipt record --stop; wait $rp
  check "--audio with --gif says GIFs have no sound, and still makes one" 'grep -q "GIFs have no sound" /tmp/snd.err && [ "$(head -c 6 /tmp/snd.gif)" = GIF89a ]'

  /clipt record -g "$g" --audio=mic -o /tmp/snd-x.mp4 2>/tmp/snd.err
  check "an unsupported source is refused ($(head -1 /tmp/snd.err))" 'grep -q "isn.t supported yet" /tmp/snd.err && [ ! -e /tmp/snd-x.mp4 ]'
  PULSE_SERVER=unix:/nonexistent /clipt record -g "$g" --audio -o /tmp/snd-x.mp4 2>/tmp/snd.err
  check "no sound server: refused before recording ($(head -1 /tmp/snd.err | cut -c1-70))" 'grep -q "can.t record sound" /tmp/snd.err && [ ! -e /tmp/snd-x.mp4 ]'
}
