#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

wait_for_lines() {
  local file="$1" expected="$2" i
  for i in $(seq 1 100); do
    [ -f "$file" ] && [ "$(wc -l < "$file")" -ge "$expected" ] && return 0
    sleep 0.02
  done
  return 1
}

make_common_mocks() {
  local dir="$1"
  mkdir -p "$dir"
  cat > "$dir/curl" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
args="$*"
if [[ "$args" == *'/notify/stream'* ]]; then
  echo stream >> "$MOCK_STREAM_CALLS"
  printf 'data: build\ndata: finished\n\n'
  exit 22
fi
body=$(cat)
printf '%s\n' "$args" >> "$MOCK_SAY_ARGS"
printf '%s\n' "$body" >> "$MOCK_SAY_BODIES"
if [[ "$args" == *'audio/wav'* ]]; then
  output=""
  previous=""
  for arg in "$@"; do
    if [ "$previous" = -o ]; then output="$arg"; fi
    previous="$arg"
  done
  {
    printf 'RIFF\377\377\377\377WAVEfmt '
    printf '\020\000\000\000\001\000\001\000\200\273\000\000\000\167\001\000\002\000\020\000data\377\377\377\377'
    printf '\000\000\001\000'
  } > "$output"
else
  printf 'OggS-mock-audio'
fi
MOCK
  chmod +x "$dir/curl"

  cat > "$dir/sleep" <<'MOCK'
#!/usr/bin/env bash
/bin/sleep 0.01
MOCK
  chmod +x "$dir/sleep"
}

test_ffplay_and_reconnect() {
  local dir="$TMP/ffplay"
  make_common_mocks "$dir"
  cat > "$dir/ffplay" <<'MOCK'
#!/usr/bin/env bash
cat >> "$MOCK_AUDIO"
MOCK
  chmod +x "$dir/ffplay"

  MOCK_STREAM_CALLS="$TMP/ffplay-stream" MOCK_SAY_ARGS="$TMP/ffplay-args" \
    MOCK_SAY_BODIES="$TMP/ffplay-bodies" MOCK_AUDIO="$TMP/ffplay-audio" \
    MOCA_RETRY_DELAY=0 PATH="$dir:/usr/bin:/bin" "$ROOT/bin/moca-listen" \
    > /dev/null 2> "$TMP/ffplay-err" &
  local pid=$!
  wait_for_lines "$TMP/ffplay-stream" 2 || { kill "$pid" 2>/dev/null || true; fail "SSEへ再接続しなかった"; }
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true

  grep -q 'audio/ogg' "$TMP/ffplay-args" || fail "ffplayでOggを要求していない"
  grep -q '^build$' "$TMP/ffplay-bodies" || fail "複数data行を改行付きで渡していない"
  grep -q '^finished$' "$TMP/ffplay-bodies" || fail "SSE本文が欠落した"
  grep -q 'OggS-mock-audio' "$TMP/ffplay-audio" || fail "ffplayへ音声を渡していない"
}

test_macos_native_wav() {
  local dir="$TMP/native"
  make_common_mocks "$dir"
  cat > "$dir/uname" <<'MOCK'
#!/usr/bin/env bash
[ "${1:-}" = -s ] && echo Darwin || echo Darwin
MOCK
  cat > "$dir/afplay" <<'MOCK'
#!/usr/bin/env bash
cp "${@: -1}" "$MOCK_PLAYED_WAV"
MOCK
  chmod +x "$dir/uname" "$dir/afplay"

  MOCK_STREAM_CALLS="$TMP/native-stream" MOCK_SAY_ARGS="$TMP/native-args" \
    MOCK_SAY_BODIES="$TMP/native-bodies" MOCK_PLAYED_WAV="$TMP/played.wav" \
    MOCA_PLAYER=afplay MOCA_RETRY_DELAY=0 PATH="$dir:/usr/bin:/bin" "$ROOT/bin/moca-listen" \
    > /dev/null 2> "$TMP/native-err" &
  local pid=$!
  for _ in $(seq 1 100); do
    [ -f "$TMP/played.wav" ] && break
    sleep 0.02
  done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true

  [ -f "$TMP/played.wav" ] || fail "afplayが呼ばれなかった"
  grep -q 'audio/wav' "$TMP/native-args" || fail "ネイティブ再生でWAVを要求していない"
  size=$(wc -c < "$TMP/played.wav")
  riff=$(od -An -tu4 -j4 -N4 "$TMP/played.wav" | tr -d ' ')
  data=$(od -An -tu4 -j40 -N4 "$TMP/played.wav" | tr -d ' ')
  [ "$riff" = "$((size - 8))" ] || fail "RIFFサイズを補正していない"
  [ "$data" = "$((size - 44))" ] || fail "dataサイズを補正していない"
}

test_wsl_soundplayer() {
  local dir="$TMP/wsl"
  make_common_mocks "$dir"
  cat > "$dir/wslpath" <<'MOCK'
#!/usr/bin/env bash
printf 'C:\\Temp\\moca.wav\n'
MOCK
  cat > "$dir/powershell.exe" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$MOCK_POWERSHELL_ARGS"
MOCK
  chmod +x "$dir/wslpath" "$dir/powershell.exe"

  MOCK_STREAM_CALLS="$TMP/wsl-stream" MOCK_SAY_ARGS="$TMP/wsl-args" \
    MOCK_SAY_BODIES="$TMP/wsl-bodies" MOCK_POWERSHELL_ARGS="$TMP/powershell-args" \
    MOCA_PLAYER=windows-soundplayer MOCA_RETRY_DELAY=0 \
    PATH="$dir:/usr/bin:/bin" "$ROOT/bin/moca-listen" > /dev/null 2> "$TMP/wsl-err" &
  local pid=$!
  wait_for_lines "$TMP/powershell-args" 1 || { kill "$pid" 2>/dev/null || true; fail "SoundPlayerが呼ばれなかった"; }
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true

  grep -q 'audio/wav' "$TMP/wsl-args" || fail "WSL再生でWAVを要求していない"
  grep -q 'System.Media.SoundPlayer' "$TMP/powershell-args" || fail "PowerShellでSoundPlayerを使っていない"
  grep -Fq 'C:\Temp\moca.wav' "$TMP/powershell-args" || fail "Windows形式のWAVパスを渡していない"
}

test_automatic_player_selection() {
  local dir="$TMP/select"
  mkdir -p "$dir"
  cat > "$dir/uname" <<'MOCK'
#!/bin/bash
case "${MOCK_OS:-}" in
  Darwin) echo Darwin ;;
  WSL) [ "${1:-}" = -r ] && echo 5.15.0-microsoft-standard-WSL2 || echo Linux ;;
  Linux) echo Linux ;;
esac
MOCK
  cat > "$dir/afplay" <<'MOCK'
#!/bin/bash
exit 0
MOCK
  cat > "$dir/pw-play" <<'MOCK'
#!/bin/bash
exit 0
MOCK
  cat > "$dir/paplay" <<'MOCK'
#!/bin/bash
exit 0
MOCK
  cat > "$dir/aplay" <<'MOCK'
#!/bin/bash
exit 0
MOCK
  cat > "$dir/powershell.exe" <<'MOCK'
#!/bin/bash
exit 0
MOCK
  cat > "$dir/wslpath" <<'MOCK'
#!/bin/bash
exit 0
MOCK
  chmod +x "$dir"/*

  selected=$(MOCK_OS=Darwin PATH="$dir" /bin/bash -c 'source "$1"; select_player; printf "%s" "$PLAYER"' _ "$ROOT/bin/moca-listen")
  [ "$selected" = afplay ] || fail "macOSでafplayを自動選択しなかった"
  selected=$(MOCK_OS=Linux PATH="$dir" /bin/bash -c 'source "$1"; select_player; printf "%s" "$PLAYER"' _ "$ROOT/bin/moca-listen")
  [ "$selected" = pw-play ] || fail "Linuxでpw-playを優先しなかった"
  selected=$(MOCK_OS=WSL PATH="$dir" /bin/bash -c 'source "$1"; select_player; printf "%s" "$PLAYER"' _ "$ROOT/bin/moca-listen")
  [ "$selected" = windows-soundplayer ] || fail "WSLでSoundPlayerを自動選択しなかった"
}

test_missing_player_fails() {
  local dir="$TMP/missing"
  mkdir -p "$dir"
  cat > "$dir/uname" <<'MOCK'
#!/usr/bin/env bash
echo UnknownOS
MOCK
  chmod +x "$dir/uname"
  if MOCA_PLAYER=not-a-player PATH="$dir:/usr/bin:/bin" "$ROOT/bin/moca-listen" > /dev/null 2> "$TMP/missing-err"; then
    fail "プレイヤーなしで成功終了した"
  fi
  grep -q 'ffmpeg.*インストール' "$TMP/missing-err" || fail "ffmpegの導入案内がない"
}

volume_mock_dir() {
  local dir="$1"
  make_common_mocks "$dir"
  cat > "$dir/ffplay" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$MOCK_PLAYER_ARGS"
cat > /dev/null
MOCK
  cat > "$dir/pw-play" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$MOCK_PLAYER_ARGS"
MOCK
  cat > "$dir/paplay" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$MOCK_PLAYER_ARGS"
MOCK
  cat > "$dir/afplay" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$MOCK_PLAYER_ARGS"
MOCK
  cat > "$dir/aplay" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$MOCK_PLAYER_ARGS"
MOCK
  chmod +x "$dir"/ffplay "$dir"/pw-play "$dir"/paplay "$dir"/afplay "$dir"/aplay
}

# 1 回だけ起動して player の引数行を 1 行取る。$1=name $2=player、残りは moca-listen の引数。
# 環境変数は呼び出し側で `MOCA_VOLUME=30 run_player_once ...` のように付ける。
run_player_once() {
  local name="$1" player="$2"
  shift 2
  local dir="$TMP/volume-$name"
  volume_mock_dir "$dir"
  MOCK_STREAM_CALLS="$TMP/$name-stream" MOCK_SAY_ARGS="$TMP/$name-args" \
    MOCK_SAY_BODIES="$TMP/$name-bodies" MOCK_PLAYER_ARGS="$TMP/$name-player" \
    MOCA_PLAYER="$player" MOCA_RETRY_DELAY=0 PATH="$dir:/usr/bin:/bin" \
    "$ROOT/bin/moca-listen" "$@" > /dev/null 2> "$TMP/$name-err" &
  local pid=$!
  wait_for_lines "$TMP/$name-player" 1 || { kill "$pid" 2>/dev/null || true; fail "$name: player が呼ばれなかった"; }
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  head -1 "$TMP/$name-player"
}

test_volume_is_passed_to_players() {
  local line
  line=$(run_player_once default ffplay)
  [[ "$line" == *'-volume 100'* ]] || fail "既定で ffplay に -volume 100 を渡していない: $line"
  line=$(run_player_once cli ffplay --volume 30)
  [[ "$line" == *'-volume 30'* ]] || fail "--volume 30 が ffplay に届いていない: $line"
  line=$(MOCA_VOLUME=30 run_player_once envvar ffplay)
  [[ "$line" == *'-volume 30'* ]] || fail "MOCA_VOLUME=30 が ffplay に届いていない: $line"
  line=$(MOCA_VOLUME=50 run_player_once priority ffplay --volume 30)
  [[ "$line" == *'-volume 30'* ]] || fail "引数が環境変数より優先されていない: $line"
  line=$(run_player_once leadzero ffplay --volume 030)
  [[ "$line" == *'-volume 30'* ]] || fail "先頭 0 付き (030) を 10 進として扱っていない: $line"
  line=$(run_player_once equals ffplay --volume=30)
  [[ "$line" == *'-volume 30'* ]] || fail "--volume=30 の形が効いていない: $line"
  line=$(MOCA_VOLUME=05 run_player_once leadzero-pw pw-play)
  [[ "$line" == *'--volume 0.05'* ]] || fail "MOCA_VOLUME=05 で pw-play に 0.05 を渡していない: $line"
  line=$(MOCA_VOLUME=30 run_player_once pwplay pw-play)
  [[ "$line" == *'--volume 0.30'* ]] || fail "pw-play に --volume 0.30 を渡していない: $line"
  line=$(MOCA_VOLUME=30 run_player_once paplay paplay)
  [[ "$line" == *'--volume 19660'* ]] || fail "paplay に --volume 19660 を渡していない: $line"
  line=$(MOCA_VOLUME=30 run_player_once afplay afplay)
  [[ "$line" == *'-v 0.30'* ]] || fail "afplay に -v 0.30 を渡していない: $line"
  line=$(MOCA_VOLUME=30 run_player_once aplay aplay)
  [[ "$line" != *'volume'* ]] || fail "aplay に音量引数を渡してしまった: $line"
  [ "$(grep -c '音量指定に対応しない' "$TMP/aplay-err")" = 1 ] || fail "aplay の音量非対応 warning が 1 回ではない"
  ! grep -q '音量指定に対応しない' "$TMP/default-err" || fail "既定 100 で warning を出している"
}

test_volume_rejects_bad_values() {
  local dir="$TMP/volume-bad" value
  volume_mock_dir "$dir"
  for value in 0 101 abc 1e1 -5 "" 000000000000000000000000030; do
    if MOCA_PLAYER=ffplay PATH="$dir:/usr/bin:/bin" "$ROOT/bin/moca-listen" --volume "$value" > /dev/null 2> "$TMP/bad-err"; then
      fail "--volume $value が成功終了した"
    else
      [ "$?" = 2 ] || fail "--volume $value の exit code が 2 ではない"
    fi
    grep -q -- '--volume' "$TMP/bad-err" || fail "--volume $value で usage が出ていない"
  done
  if MOCA_PLAYER=ffplay MOCA_VOLUME=abc PATH="$dir:/usr/bin:/bin" "$ROOT/bin/moca-listen" > /dev/null 2> "$TMP/bad-env-err"; then
    fail "MOCA_VOLUME=abc が成功終了した"
  else
    [ "$?" = 2 ] || fail "MOCA_VOLUME=abc の exit code が 2 ではない"
  fi
  PATH="$dir:/usr/bin:/bin" "$ROOT/bin/moca-listen" --help > "$TMP/help-out" 2>&1 || fail "--help が 0 で終わらない"
  grep -q -- '--volume' "$TMP/help-out" || fail "--help に --volume が無い"
}

test_ffplay_and_reconnect
test_macos_native_wav
test_wsl_soundplayer
test_automatic_player_selection
test_missing_player_fails
test_volume_is_passed_to_players
test_volume_rejects_bad_values
echo "moca-listen tests: ok"
