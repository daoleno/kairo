#!/bin/sh
# The web build end to end, as a person meets it, in a user + network
# namespace (no root, nothing on this machine's network is touched):
# first run with the setup code, a subscription by link, picking an exit and
# a request leaving through that very server, Every app on and off, a phone
# signing in, the desktop signing it out, a restart keeping the session, and
# no frame carrying the subscription's token.
#
#   scripts/web-e2e.sh dist/Kairo-server-<v>-linux-<arch> OUT_DIR
#
# Needs node, curl, Chrome (CHROME=...) and playwright-core
# (PLAYWRIGHT=/path/to/node_modules/playwright-core/index.mjs, or installed
# where node finds it). Screenshots and results.json land in OUT_DIR.
set -e
pkg=$(cd "$1" && pwd)
out=$2
[ -x "$pkg/bin/kairo-server" ] || { echo "usage: $0 <unpacked Kairo-server dir> <out dir>" >&2; exit 1; }
mkdir -p "$out"
here=$(cd "$(dirname "$0")/web-e2e" && pwd)
# Not this machine's desktop session (its system proxy): the namespace's own.
unset DBUS_SESSION_BUS_ADDRESS
# 10.66.0.1 on lo: the server as another machine on the network reaches it.
exec unshare -rn sh -c 'ip link set lo up && ip addr add 10.66.0.1/32 dev lo && exec node "$1/e2e.mjs" "$2" "$3"' sh "$here" "$pkg" "$(cd "$out" && pwd)"
