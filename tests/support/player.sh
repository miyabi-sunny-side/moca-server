#!/usr/bin/env bash
# Isolated player double; never opens an audio device.
set -eu
if [[ ${0##*/} == ffplay ]]; then
  cat > /dev/null
  if [[ " $* " != *' -nostats '* ]]; then
    printf '\n'
    printf '\033[2K\r' >&2
  fi
fi
printf '%s\0' "$@" >> "$PLAYER_EVENTS"
printf '\0' >> "$PLAYER_EVENTS"
if [[ -n ${PLAYER_GATE:-} ]]; then
  while [[ ! -e $PLAYER_GATE ]]; do sleep 0.01; done
fi
if [[ -n ${PLAYER_FAIL:-} ]]; then
  printf 'player: device unavailable\n' >&2
  exit "${PLAYER_FAIL_STATUS:-7}"
fi
