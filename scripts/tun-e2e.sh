#!/bin/sh
# The TUN end-to-end test, in a user + network namespace (no root, no change
# to this machine's network). Needs a core: KAIRO_TEST_CORE=/path/to/mihomo.
set -e
cd "$(dirname "$0")/.."
: "${KAIRO_TEST_CORE:?set KAIRO_TEST_CORE to a mihomo binary}"
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
