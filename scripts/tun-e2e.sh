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
KAIRO_TEST_TUN=1 KAIRO_TEST_CORE="$KAIRO_TEST_CORE" exec unshare -rn "$bin" tun_in_a_network_namespace --nocapture
