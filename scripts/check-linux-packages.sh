#!/bin/sh
# scripts/check-linux-packages.sh <dist>
# The Linux packages in <dist> (this machine's architecture), each in a clean
# ubuntu:24.04 container (docker), so nothing a build machine already has hides
# a missing library:
# - the .deb alone, through apt, brings every library Kairo links;
# - the .tar.gz, with only the libraries the README names, installs with its
#   ./install.sh and finds every library it links.
# Nothing on the machine that runs it changes.
set -e
dist=$(cd "$1" && pwd)
deb=$(cd "$dist" && ls kairo_*.deb | head -1)
tgz=$(cd "$dist" && ls Kairo-[0-9]*-linux-*.tar.gz | head -1)
[ -n "$deb" ] && [ -n "$tgz" ] || { echo "no .deb or .tar.gz in $dist" >&2; exit 1; }

# Every ELF file given whose libraries are not all found, and which ones.
lddcheck='
missing=0
for f in "$@"; do
  out=$(ldd "$f" 2>&1) || continue   # not a dynamic executable (the Go core)
  if echo "$out" | grep -q "not found"; then echo "$f:"; echo "$out" | grep "not found"; missing=1; fi
done
exit $missing'

echo "== $deb alone, in a clean Ubuntu 24.04"
docker run --rm -v "$dist:/d:ro" -e DEBIAN_FRONTEND=noninteractive ubuntu:24.04 sh -ec "
  apt-get update -qq
  apt-get install -y -qq --no-install-recommends /d/$deb > /tmp/apt.log 2>&1 || { tail -30 /tmp/apt.log; exit 1; }
  dpkg -s kairo | grep -E '^(Version|Depends):'
  sh -c '$lddcheck' - /usr/bin/kairo /usr/lib/kairo/*
  /usr/lib/kairo/mihomo -v
  echo 'every library found'
  # It starts on a screen (Xvfb) in a scratch home, stays up, and writes its settings.
  apt-get install -y -qq --no-install-recommends xvfb > /tmp/apt.log 2>&1 || { tail -30 /tmp/apt.log; exit 1; }
  mkdir -p /tmp/home /tmp/rt && chmod 700 /tmp/rt
  export KAIRO_HOME=/tmp/home KAIRO_NO_SYSPROXY=1 KAIRO_NO_UPDATE=1 XDG_RUNTIME_DIR=/tmp/rt RUST_LOG=info DISPLAY=:9
  Xvfb :9 -screen 0 1400x900x24 > /dev/null 2>&1 &
  sleep 2
  kairo > /tmp/kairo.log 2>&1 &
  pid=\$!
  sleep 15
  kill -0 \$pid 2>/dev/null || { echo 'kairo did not stay up:'; tail -30 /tmp/kairo.log; exit 1; }
  test -s /tmp/home/settings.json || { echo 'no settings.json'; tail -30 /tmp/kairo.log; exit 1; }
  ! grep -q panicked /tmp/kairo.log || { tail -30 /tmp/kairo.log; exit 1; }
  kill \$pid
  echo 'kairo started on a screen and stayed up'
"

echo "== $tgz with the README's libraries, in a clean Ubuntu 24.04"
docker run --rm -v "$dist:/d:ro" -e DEBIAN_FRONTEND=noninteractive ubuntu:24.04 sh -ec "
  apt-get update -qq
  apt-get install -y -qq --no-install-recommends libgtk-4-1 libadwaita-1-0 libwebkitgtk-6.0-4 libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 > /tmp/apt.log 2>&1 || { tail -30 /tmp/apt.log; exit 1; }
  cd /tmp && tar -xzf /d/$tgz && cd Kairo-*/
  ./install.sh
  sh -c '$lddcheck' - \$HOME/.local/bin/kairo \$HOME/.local/lib/kairo/*
  \$HOME/.local/lib/kairo/mihomo -v
  echo 'every library found'
"
