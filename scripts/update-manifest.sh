#!/bin/sh
# scripts/update-manifest.sh <dir> <version>
# Writes <dir>/kairo-update.json, the manifest Kairo's updater reads, from the
# packages in <dir> and their lines in <dir>/SHA256SUMS. The release workflow
# signs it (kairo-update.json.sig); see crates/kairo-core/src/update.rs.
set -e
dir=$1
version=$2
[ -s "$dir/SHA256SUMS" ] && [ -n "$version" ] || { echo "usage: $0 <dir with SHA256SUMS> <version>" >&2; exit 1; }
python3 -I - "$dir" "$version" <<'PY'
import json, sys
d, v = sys.argv[1], sys.argv[2]
names = {
    "linux-x86_64-tar": f"Kairo-{v}-linux-x86_64.tar.gz",
    "linux-aarch64-tar": f"Kairo-{v}-linux-aarch64.tar.gz",
    "linux-x86_64-deb": f"kairo_{v}_amd64.deb",
    "linux-aarch64-deb": f"kairo_{v}_arm64.deb",
    # kairo-server: the engine and its page, no GTK.
    "linux-x86_64-server-tar": f"Kairo-server-{v}-linux-x86_64.tar.gz",
    "linux-aarch64-server-tar": f"Kairo-server-{v}-linux-aarch64.tar.gz",
    "macos-universal-zip": f"Kairo-{v}-macos-universal.zip",
    "windows-x86_64-zip": f"Kairo-{v}-windows-x86_64.zip",
    # Android: the arm64 phone APK (the update is a whole APK the system installs).
    "android-arm64-apk": f"Kairo-{v}-android-arm64-v8a.apk",
}
sums = {}
for line in open(f"{d}/SHA256SUMS"):
    parts = line.split()
    if len(parts) == 2:
        sums[parts[1].lstrip("*")] = parts[0]
packages = {}
for key, name in names.items():
    if name not in sums:
        sys.exit(f"{name} is not in SHA256SUMS")
    packages[key] = {"name": name, "sha256": sums[name]}
manifest = {"version": v, "packages": packages}
with open(f"{d}/kairo-update.json", "w") as f:
    json.dump(manifest, f, indent=2, sort_keys=True)
    f.write("\n")
print(json.dumps(manifest, indent=2, sort_keys=True))
PY
