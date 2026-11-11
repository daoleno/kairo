# dist\Kairo-<version>-windows-x86_64.zip and ...-setup.exe: Kairo's own program
# (window, tray, guardian) in release mode with the pinned mihomo core
# (scripts\mihomo.env, checksum verified) beside it, and the licences.
# The zip is what Kairo updates itself from; the setup installs it for the
# current user (no administrator). Run it on Windows with Rust, Git Bash and
# Inno Setup (iscc) installed.
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value
$name = "Kairo-$version-windows-x86_64"
$stage = "dist\stage\Kairo"
Remove-Item -Recurse -Force "dist\stage" -ErrorAction SilentlyContinue
Remove-Item -Force "dist\$name.zip", "dist\$name-setup.exe" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
Copy-Item target\release\kairo.exe "$stage\kairo.exe"
# What gives the core the rights TUN needs (a scheduled task or service runs it).
Copy-Item target\release\kairo-core-host.exe "$stage\kairo-core-host.exe"

# The core and its licence.
bash scripts/fetch-mihomo.sh windows-x86_64 "dist/stage/Kairo"
if ($LASTEXITCODE -ne 0) { throw "fetching the core failed" }
Move-Item "$stage\LICENSE.mihomo" "$stage\LICENSE.mihomo.txt"
$pinned = (Select-String -Path scripts\mihomo.env -Pattern '^MIHOMO_VERSION=(.*)').Matches.Groups[1].Value
$reported = & "$stage\mihomo.exe" -v | Select-Object -First 1
if ($reported -notlike "Mihomo Meta $pinned *") { throw "the core reports '$reported', expected $pinned" }
# The wintun driver library (TUN) and its licence.
bash scripts/fetch-wintun.sh "dist/stage/Kairo"
if ($LASTEXITCODE -ne 0) { throw "fetching wintun failed" }
$wintun = (Select-String -Path scripts\mihomo.env -Pattern '^WINTUN_VERSION=(.*)').Matches.Groups[1].Value
Copy-Item LICENSE "$stage\LICENSE.txt"
@"
Kairo ${version}: https://github.com/daoleno/kairo
Free software under the GNU GPL, version 3 or later (LICENSE.txt).

It ships the mihomo core $pinned (mihomo.exe), the official MetaCubeX
build, unmodified. mihomo is free software under the GNU GPL, version 3
(LICENSE.mihomo.txt). Its source code:
https://github.com/MetaCubeX/mihomo/tree/$pinned

For TUN ("Every app") it ships wintun.dll $wintun, the official signed build
from https://www.wintun.net, unmodified, under the Prebuilt Binaries License
(LICENSE.wintun.txt), and kairo-core-host.exe, part of Kairo, which runs the
core when Windows is asked to start it.
"@ | Set-Content -Encoding utf8 "$stage\NOTICE.txt"

New-Item -ItemType Directory -Force dist | Out-Null
Compress-Archive -Path "$stage" -DestinationPath "dist\$name.zip"

iscc /Qp "/DAppVersion=$version" "/DSourceDir=$((Resolve-Path $stage).Path)" "/DOutputName=$name-setup" "/O$((Resolve-Path dist).Path)" windows\kairo.iss
if ($LASTEXITCODE -ne 0) { throw "iscc failed" }
Remove-Item -Recurse -Force "dist\stage"
Get-ChildItem dist | Where-Object { $_.Name -like "$name*" } | ForEach-Object { "$($_.Name)  $($_.Length)" }
