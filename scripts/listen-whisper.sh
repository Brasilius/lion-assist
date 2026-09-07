#!/usr/bin/env bash
# Linux microphone adapter for voice.listen_command. Output is transcript only.
set -euo pipefail
: "${LION_WHISPER_MODEL:?Set LION_WHISPER_MODEL to a local whisper.cpp GGML model}"
: "${LION_VOICE_TMP:?Set LION_VOICE_TMP to a private scratch directory inside your storage boundary}"
[[ -f "$LION_WHISPER_MODEL" && -d "$LION_VOICE_TMP" ]]
umask 077
voice_capture_dir=$(mktemp -d "$LION_VOICE_TMP/capture.XXXXXXXX")
trap 'rm -rf -- "$voice_capture_dir"' EXIT
arecord -q -D "${LION_MIC_DEVICE:-default}" -f S16_LE -r 16000 -c 1 -d 5 "$voice_capture_dir/audio.wav"
whisper-cli -m "$LION_WHISPER_MODEL" -f "$voice_capture_dir/audio.wav" -l en -nt -otxt -of "$voice_capture_dir/transcript" >/dev/null 2>&1
cat "$voice_capture_dir/transcript.txt"
