#!/bin/sh
# `nsh test` with the app against a real core and the fixture, in a scratch
# home on ports found free when it starts (so two runs, or another client,
# never collide). The tests learn them from tests/ports.ts, which this
# script writes for the run and puts back at the end. The core:
# KAIRO_TEST_CORE, else target/mihomo/mihomo (scripts/fetch-mihomo.sh).
# Never touches the system proxy.
#
# It runs in a user + network namespace of its own (unshare -rn, as
# scripts/tun-e2e.sh): only `lo`, with 127.0.0.1 and PUBLIC (203.0.113.10, a
# documentation address no network uses). The fixture answers there too, so a
# provider's notice comes from an address Kairo does not count as this
# machine's (from one that is, it shows no provider's words). Nothing in it
# reaches the internet: everything is built first, outside it (a run that
# matches no test), and the run inside is offline. Delays are measured against
# the fixture's /generate_204 (testUrl), not a site on the internet.
set -e
cd "$(dirname "$0")/.."
PUBLIC=203.0.113.10
if [ -z "$KAIRO_UI_NETNS" ]; then
  # The build alone (a filter no test matches), in a home of its own all the same:
  # an app that starts must never meet this machine's Kairo or its core.
  build_home=$(mktemp -d "${TMPDIR:-/tmp}/kairo-ui-build.XXXXXX")
  for f in tests/*.test.tsx; do
    KAIRO_HOME="$build_home" KAIRO_NO_SYSPROXY=1 KAIRO_NO_UPDATE=1 KAIRO_NO_ICON_FETCH=1 \
      nsh test --filter "nothing is named this: the build alone" "$f" >/dev/null
  done
  rm -rf "$build_home"
  unshare -rn true 2>/dev/null || {
    echo "no user + network namespace (Ubuntu 24.04: sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0)" >&2
    exit 1
  }
  KAIRO_UI_NETNS=1 exec unshare -rn sh "$PWD/scripts/ui-test.sh" "$@"
fi
ip link set lo up
ip addr add "$PUBLIC/32" dev lo
export CARGO_NET_OFFLINE=true
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
export const PUBLIC = "$PUBLIC";
TS
cat > "$home/settings.json" <<JSON
{"mixedPort": $mixed, "controllerPort": $controller, "corePath": "$KAIRO_TEST_CORE", "testTimeoutMs": 2000, "autoLocation": true, "testUrl": "http://$PUBLIC:$fixture_port/generate_204"}
JSON
python3 -I scripts/fixture.py --port "$fixture_port" --public "$PUBLIC" > "$home/fixture.log" 2>&1 &
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
# The test files, one after another on the one home (the first adds the subscription
# the later ones see); the crates' own tests (cargo test) need the internet and run on their own.
failed=0
for f in tests/*.test.tsx; do
  KAIRO_HOME="$home" KAIRO_NO_SYSPROXY=1 KAIRO_NO_UPDATE=1 KAIRO_NO_ICON_FETCH=1 nsh test "$@" "$f" || failed=1
done
exit $failed
