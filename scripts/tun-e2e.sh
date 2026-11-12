#!/bin/sh
# The TUN end-to-end test, in a user + network namespace (no root, no change
# to this machine's network). The core: KAIRO_TEST_CORE, else
# target/mihomo/mihomo (scripts/fetch-mihomo.sh).
set -e
cd "$(dirname "$0")/.."
KAIRO_TEST_CORE=${KAIRO_TEST_CORE:-$PWD/target/mihomo/mihomo}
[ -x "$KAIRO_TEST_CORE" ] || { echo "no core: run scripts/fetch-mihomo.sh or set KAIRO_TEST_CORE" >&2; exit 1; }
bin=$(cargo test -p kairo-core --test e2e --no-run --message-format=json 2>/dev/null \
  | python3 -c 'import sys, json
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "e2e":
        print(m["executable"])')
# The namespace gets its own resolv.conf (a private mount namespace), with a
# nameserver only TUN's DNS hijack can answer. A host resolver on loopback
# (systemd-resolved's 127.0.0.53) would not be reachable in there.
resolv=$(mktemp)
trap 'rm -f "$resolv"' EXIT
echo "nameserver 203.0.113.53" > "$resolv"
KAIRO_TEST_TUN=1 KAIRO_TEST_CORE="$KAIRO_TEST_CORE" unshare -rnm sh -c \
  'mount --bind "$1" /etc/resolv.conf && exec "$2" tun_in_a_network_namespace --nocapture' sh "$resolv" "$bin"

# "Allow other devices" against the real core, and subscriptions that lead to
# this machine: the namespace's loopback also carries 10.77.0.1 (another
# device on the network), 10.77.0.2 (a server on it) and 203.0.113.5 (one out
# on the internet, as Kairo counts it).
lan=$(cargo test -p kairo-core --test lan --no-run --message-format=json 2>/dev/null \
  | python3 -c 'import sys, json
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "lan":
        print(m["executable"])')
KAIRO_TEST_NETNS=1 KAIRO_TEST_CORE="$KAIRO_TEST_CORE" unshare -rn sh -c \
  'ip link set lo up && ip addr add 10.77.0.1/32 dev lo && ip addr add 10.77.0.2/32 dev lo && ip addr add 203.0.113.5/32 dev lo && exec "$1" --test-threads=1 --nocapture' sh "$lan"

# Both at once (this machine's own setup): TUN and "Allow other devices", in a
# namespace of its own whose resolv.conf only TUN's DNS hijack answers.
KAIRO_TEST_NETNS=1 KAIRO_TEST_TUN=1 KAIRO_TEST_CORE="$KAIRO_TEST_CORE" unshare -rnm sh -c \
  'mount --bind "$1" /etc/resolv.conf && ip link set lo up && ip addr add 10.77.0.1/32 dev lo && ip addr add 10.77.0.2/32 dev lo && exec "$2" with_tun_on_this_machines_names_reach_its_loopback_and_other_devices_still_do_not --exact --nocapture' sh "$resolv" "$lan"
