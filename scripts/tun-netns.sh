#!/bin/sh
# TUN without touching the host: run Kairo in a user + network namespace of
# its own (uid 0 inside, so the core may create the TUN device and routes).
#
#   unshare -rn scripts/tun-netns.sh            # starts the fixture and Kairo
#   nsenter -t <kairo pid> -U -n --preserve-credentials curl http://203.0.113.10:18080/hello
#
# Only `lo` exists in the namespace, so nothing gets out until TUN is on;
# with it on, flows go through the core to the fixture's proxy nodes.
set -e
ip link set lo up
cd "$(dirname "$0")/.."
: "${KAIRO_HOME:=${TMPDIR:-/tmp}/kairo-tun}"
python3 -I scripts/fixture.py --port 18080 > "${TMPDIR:-/tmp}/kairo-tun-fixture.log" 2>&1 &
sleep 1
KAIRO_HOME="$KAIRO_HOME" KAIRO_NO_SYSPROXY=1 exec ./target/debug/kairo
