#!/bin/sh
# `nsh test` with the app against a real core and the fixture, in a scratch
# home on ports of its own (27897 mixed, 29097 controller). Needs a core:
# KAIRO_TEST_CORE=/path/to/mihomo. Never touches the system proxy.
set -e
cd "$(dirname "$0")/.."
: "${KAIRO_TEST_CORE:?set KAIRO_TEST_CORE to a mihomo binary}"
home=$(mktemp -d "${TMPDIR:-/tmp}/kairo-ui-test.XXXXXX")
cat > "$home/settings.json" <<JSON
{"mixedPort": 27897, "controllerPort": 29097, "corePath": "$KAIRO_TEST_CORE", "testTimeoutMs": 2000}
JSON
python3 -I scripts/fixture.py --port 28080 > "$home/fixture.log" 2>&1 &
fixture=$!
trap 'kill $fixture 2>/dev/null; [ -n "$KEEP" ] || rm -rf "$home"' EXIT INT TERM
sleep 1
KAIRO_HOME="$home" KAIRO_NO_SYSPROXY=1 nsh test "$@"
