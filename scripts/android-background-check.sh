#!/bin/sh
# scripts/android-background-check.sh [swipe|kill|tile|reboot|all]
# Kairo on Android stays on without its screen: after the app is swiped away, after
# Android kills its process (as a low-memory kill does), and after a restart of the
# phone; and the Quick Settings tile turns it off and on, also with the app's
# process gone (a real tap on the tile in the shade). Each check is a real request
# from the device through the VPN: the host
# kairo-probe.test only resolves and answers through Kairo's core (the fixture's
# proxies map *.test to its origin), so a pass is traffic through the tunnel.
#
# Needs: one device or emulator on adb (ANDROID_SERIAL picks one) with a debug build
# of Kairo (scripts/package-android.sh, for run-as) that is connected with the
# fixture's subscription, http://127.0.0.1:18080/sub (added once in the app; the
# VPN permission given once; for `tile`, the tile added to Quick Settings: Kairo's
# Settings > Quick Settings > Add). The fixture runs here and the device reaches it
# through `adb reverse`, which this script sets up (again after a reboot).
#
#   python3 -I scripts/fixture.py --port 18080 &   # started here if not running
set -e
here=$(cd "$(dirname "$0")/.." && pwd)
what=${1:-all}
pkg=dev.kairo.app
port=18080
ports="18080 18081 18082 18083 18084 18085 18086"

fixture_pid=
cleanup() { [ -z "$fixture_pid" ] || kill "$fixture_pid" 2>/dev/null || true; }
trap cleanup EXIT
if ! python3 -I -c "import socket,sys; socket.create_connection(('127.0.0.1', $port), 1)" 2>/dev/null; then
  python3 -I "$here/scripts/fixture.py" --port $port >/dev/null 2>&1 &
  fixture_pid=$!
  sleep 1
fi

reverse() {
  for p in $ports; do adb reverse tcp:$p tcp:$p >/dev/null; done
}

# One request through the tunnel; 0 when the origin answered.
probe() {
  adb shell "(printf 'GET /hello HTTP/1.0\r\nHost: kairo-probe.test\r\n\r\n'; sleep 2) | nc -w 6 kairo-probe.test $port 2>&1" \
    | tr -d '\r' | grep -q kairo-ok
}

# Waits up to $1 seconds for a probe to pass; prints the seconds it took.
until_through() {
  start=$(date +%s)
  while :; do
    if probe; then echo $(( $(date +%s) - start )); return 0; fi
    [ $(( $(date +%s) - start )) -lt "$1" ] || return 1
    sleep 1
  done
}

pid() { adb shell pidof $pkg 2>/dev/null | tr -d '\r'; }

fail=0
check() { # name, result
  if [ "$2" = ok ]; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi
}

adb get-state >/dev/null
reverse
if ! s=$(until_through 20); then
  echo "Kairo is not connected through the fixture (add http://127.0.0.1:$port/sub in the app and connect)" >&2
  exit 2
fi
echo "ok    connected (a request through the tunnel)"

if [ "$what" = swipe ] || [ "$what" = all ]; then
  # The task out of Recents, as a swipe does (its root task removed).
  task=$(adb shell dumpsys activity recents | sed -n "s/.*Recent #[0-9]*: Task{[0-9a-f]* #\([0-9]*\) .*A=[0-9]*:$pkg.*/\1/p" | head -1 | tr -d '\r')
  if [ -z "$task" ]; then
    adb shell monkey -p $pkg -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1
    sleep 3
    task=$(adb shell dumpsys activity recents | sed -n "s/.*Recent #[0-9]*: Task{[0-9a-f]* #\([0-9]*\) .*A=[0-9]*:$pkg.*/\1/p" | head -1 | tr -d '\r')
  fi
  before=$(pid)
  adb shell am stack remove "$task" >/dev/null 2>&1 || true
  adb shell input keyevent KEYCODE_HOME
  sleep 5
  if s=$(until_through 10); then
    check "swiped away: still through the tunnel (process $before -> $(pid))" ok
  else
    check "swiped away: no traffic through the tunnel" fail
  fi
fi

if [ "$what" = kill ] || [ "$what" = all ]; then
  # What the low-memory killer does: SIGKILL, no warning.
  before=$(pid)
  adb shell run-as $pkg kill -9 "$before"
  sleep 1
  if s=$(until_through 30); then
    check "killed (pid $before): back by itself in ${s}s+1s (pid $(pid))" ok
  else
    check "killed (pid $before): not back within 30s" fail
  fi
fi

if [ "$what" = tile ] || [ "$what" = all ]; then
  tile=$pkg/.KairoTile
  # A real tap on the tile in the expanded shade, found by its label (paging
  # through the panel); `cmd statusbar click-tile` does not reach the tile.
  tap_tile() {
    adb shell cmd statusbar expand-settings >/dev/null 2>&1
    sleep 2
    for page in 1 2 3 4 5; do
      adb shell uiautomator dump /sdcard/kairo-ui.xml >/dev/null 2>&1
      at=$(adb shell cat /sdcard/kairo-ui.xml | tr '>' '\n' | grep 'text="Kairo, ' | head -1 \
        | sed -n 's/.*bounds="\[\([0-9]*\),\([0-9]*\)\]\[\([0-9]*\),\([0-9]*\)\]".*/\1 \2 \3 \4/p')
      if [ -n "$at" ]; then
        set -- $at
        adb shell input tap $(( ($1 + $3) / 2 )) $(( ($2 + $4) / 2 ))
        sleep 1
        adb shell cmd statusbar collapse >/dev/null 2>&1
        return 0
      fi
      adb shell input swipe 900 1000 150 1000 300
      sleep 1
    done
    echo "no Kairo tile in Quick Settings (add it: Settings > Quick Settings > Add)" >&2
    adb shell cmd statusbar collapse >/dev/null 2>&1
    return 1
  }
  tap_tile
  sleep 2
  if probe; then check "tile off: still through the tunnel" fail; else check "tile off: no tunnel" ok; fi
  tap_tile
  if s=$(until_through 20); then check "tile on: through the tunnel in ${s}s" ok; else check "tile on: no tunnel within 20s" fail; fi
  # Off, then the process gone (nothing restarts it: Kairo is off): the tile starts it alone.
  tap_tile
  sleep 2
  if probe; then check "tile off again: still through the tunnel" fail; fi
  before=$(pid)
  [ -z "$before" ] || adb shell run-as $pkg kill -9 "$before"
  sleep 2
  tap_tile
  if s=$(until_through 30); then
    if adb shell dumpsys activity activities | grep -E "topResumedActivity|ResumedActivity:" | grep -q $pkg; then
      check "tile on with the app's process gone: it opened Kairo" fail
    fi
    check "tile on with the app's process gone (pid $before): through the tunnel in ${s}s, no screen opened" ok
  else
    check "tile on with the app's process gone: no tunnel within 30s" fail
  fi
fi

if [ "$what" = reboot ] || [ "$what" = all ]; then
  adb reboot
  adb wait-for-device
  while [ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" != 1 ]; do sleep 2; done
  boot=$(date +%s)
  # Not before the device is unlocked once: Kairo's files are in credential-encrypted storage.
  adb shell wm dismiss-keyguard >/dev/null 2>&1 || true
  reverse
  if s=$(until_through 120); then
    check "rebooted: on again by itself $(( $(date +%s) - boot ))s after boot" ok
  else
    check "rebooted: not on within 120s of boot" fail
  fi
fi
exit $fail
