#!/bin/sh
# `nsh test` with the app against a real core and the fixture, in a scratch
# home on ports found free when it starts (so two runs, or another client,
# never collide). The tests learn them from tests/ports.ts, which this
# script writes for the run and puts back at the end. The core:
# KAIRO_TEST_CORE, else target/mihomo/mihomo (scripts/fetch-mihomo.sh).
# Never touches the system proxy.
set -e
cd "$(dirname "$0")/.."
KAIRO_TEST_CORE=${KAIRO_TEST_CORE:-$PWD/target/mihomo/mihomo}
[ -x "$KAIRO_TEST_CORE" ] || { echo "no core: run scripts/fetch-mihomo.sh or set KAIRO_TEST_CORE" >&2; exit 1; }
home=$(mktemp -d "${TMPDIR:-/tmp}/kairo-ui-test.XXXXXX")
# A block of 8 ports for the fixture (the origin, six proxies, a dead one),
# then two more: the core's mixed port and its controller.
set -- $(python3 -I - <<'PY'
import random, socket
def free(ps):
    socks = []
    try:
        for p in ps:
            s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 0)
            s.bind(("127.0.0.1", p)); socks.append(s)
        return True
    except OSError:
        return False
    finally:
        for s in socks: s.close()
while True:
    base = random.randrange(20000, 40000)
    a, b = random.randrange(40001, 60000), random.randrange(40001, 60000)
    if a != b and free(range(base, base + 8)) and free([a]) and free([b]):
        print(base, a, b); break
PY
) "$@"
fixture_port=$1; mixed=$2; controller=$3; shift 3
cp tests/ports.ts "$home/ports.ts.orig"
cat > tests/ports.ts <<TS
// Written by scripts/ui-test.sh for one run; the committed values are only defaults.
export const FIXTURE = $fixture_port;
export const MIXED = $mixed;
TS
cat > "$home/settings.json" <<JSON
{"mixedPort": $mixed, "controllerPort": $controller, "corePath": "$KAIRO_TEST_CORE", "testTimeoutMs": 2000, "autoLocation": true}
JSON
python3 -I scripts/fixture.py --port "$fixture_port" > "$home/fixture.log" 2>&1 &
fixture=$!
# The core outlives the app that started it (as after a crash): stop it too.
cleanup() {
  kill $fixture 2>/dev/null
  [ -f "$home/state/core.pid" ] && kill "$(cat "$home/state/core.pid")" 2>/dev/null
  cp "$home/ports.ts.orig" tests/ports.ts
  [ -n "$KEEP" ] || rm -rf "$home"
}
trap cleanup EXIT INT TERM
sleep 1
KAIRO_HOME="$home" KAIRO_NO_SYSPROXY=1 KAIRO_NO_UPDATE=1 KAIRO_NO_ICON_FETCH=1 nsh test "$@"
