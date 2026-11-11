#!/bin/sh
# scripts/android-version-code.sh <version>
# Android's versionCode from the version (Cargo.toml), so a release can never ship
# the same code as the one before it: major*10000 + minor*100 + patch
# (0.3.1 -> 301, 1.2.3 -> 10203). Minor and patch stay below 100.
# 0.3.1 itself shipped with code 7 (the old nsh.json "build"); every later
# version's code is larger.
set -e
v=${1:?usage: android-version-code.sh <version>}
case $v in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "not a x.y.z version: $v" >&2; exit 1 ;;
esac
python3 -I - "$v" <<'PY'
import re, sys
m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", sys.argv[1])
if not m:
    sys.exit("not a x.y.z version: " + sys.argv[1])
major, minor, patch = map(int, m.groups())
if minor > 99 or patch > 99 or major > 209:
    sys.exit("minor and patch must be below 100 (and major below 210): " + sys.argv[1])
print(major * 10000 + minor * 100 + patch)
PY
