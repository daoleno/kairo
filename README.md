# Kairo

A native desktop client for the [mihomo](https://github.com/MetaCubeX/mihomo) proxy core (Clash Meta), for
Linux (GTK 4) and macOS (AppKit), written in Rust on [NSH](https://nsh.dev).

Kairo sets up the core and keeps it running. It manages your subscriptions, lets you pick nodes, and shows live
traffic, connections, rules and logs. It can also set the system proxy or turn on TUN. The layout is dense and
calm, with one accent colour, and it follows the system's light or dark appearance.

![Overview](docs/screenshots/overview-light.png)

## What it does

- **Core**: downloads the official mihomo build for the machine and verifies it against the release's SHA-256.
  It checks for updates and can start, stop or restart the core. If the core crashes, Kairo starts it again.
  A custom core binary also works.
- **Profiles**: add a subscription URL or a YAML file. Kairo validates each one with the core before using it.
  Subscriptions update on a schedule (the provider's interval or your own) or by hand. You can see usage and
  expiry, switch profiles with one click, and keep per-profile overrides in a YAML editor. Overrides merge
  into the profile, `prepend-rules` and friends are supported, and the core checks the result before Kairo
  applies it.
- **Proxies**: see groups in config order, pick a node, and run delay tests for one node or a whole group.
  Nodes can be sorted by delay or name and filtered. Proxy providers support update and health check.
- **Modes**: Rule, Global and Direct. Switch from the Overview or the tray; the sidebar shows the current mode.
- **System proxy**: works on macOS (`networksetup`), GNOME (gsettings) and KDE (kioslaverc). The previous
  settings are put back on quit, on SIGTERM or SIGINT, on a panic, and even after a SIGKILL or crash, thanks to
  a small guardian process. The next start also recovers any leftover state. Kairo only puts back what it
  changed: if its System Proxy was never on, another app's proxy settings are left alone.
- **TUN**: one switch. The core alone gets the one-time privilege it needs: `cap_net_admin` through
  `pkexec setcap` on Linux, setuid through the administrator prompt on macOS. Kairo itself never runs
  privileged. See [TUN and privileges](#tun-and-privileges).
- **Live views**: a traffic graph, totals and memory. The connections table (sortable and searchable, with an
  inspector and close or close-all) stays smooth with thousands of rows. You also get the rules and rule
  providers, and the logs (live level, search, pause).
- **DNS tool**: query the core's resolver for A, AAAA, CNAME, MX, TXT or NS records, and flush the fake-IP cache.
- **Settings**: ports, LAN, IPv6, bypass list, TUN stack, controller and secret, delay test URL, appearance,
  launch at login, start minimized, keep running in the tray, and the tray's quick-switch group.
- **Tray / menu bar**: status and speeds, mode, system proxy, TUN, node quick switch, start/stop the core,
  show the window, and quit.

## Install

Download the latest release from [Releases](https://github.com/daoleno/kairo/releases/latest), or from the
[website](https://daoleno.github.io/kairo/).

- **macOS 11+** (Apple silicon and Intel): open the `.dmg` and drag Kairo to Applications. The app is ad hoc
  signed but not notarized, so macOS stops it the first time. Open Kairo once, then go to System Settings →
  Privacy & Security and click **Open Anyway** next to Kairo. Or run
  `xattr -dr com.apple.quarantine /Applications/Kairo.app` before opening it.
- **Debian / Ubuntu 24.04+**: `sudo apt install ./kairo_<version>_amd64.deb` (or `_arm64`).
- **Other Linux** (GTK 4.14+, WebKitGTK 6.0): unpack the `.tar.gz` and run `./install.sh`, which installs
  to `~/.local`; `./install.sh /usr/local` installs system-wide with sudo.

Check downloads against `SHA256SUMS` in the release.

## Build and run

Rust (stable) and the platform's development libraries are needed: GTK 4 on Linux, Xcode's command line tools
on macOS.

```sh
cargo run                      # the app (GTK 4 on Linux, AppKit on macOS)
nsh check                      # cargo check of every crate
KAIRO_TEST_CORE=/path/to/mihomo scripts/ui-test.sh    # nsh test: all unit, e2e and headless UI tests
KAIRO_TEST_CORE=/path/to/mihomo scripts/tun-e2e.sh    # TUN end to end, in a network namespace
scripts/package-linux.sh       # dist/: a tarball with the binary, .desktop file, icons and install.sh
scripts/package-deb.sh         # dist/: a .deb from that tree
scripts/package-macos.sh       # dist/: Kairo.app, .dmg and .zip, universal, ad hoc signed (run on a Mac)
```

Kairo keeps its data in `~/.local/share/kairo` on Linux and `~/Library/Application Support/Kairo` on macOS.
`KAIRO_HOME` overrides the location. `KAIRO_NO_SYSPROXY=1` makes Kairo leave the system proxy alone, which is
useful for development next to a proxy you rely on.

### Tests

- `cargo test -p kairo-core` runs the unit tests: config merging, subscriptions, usage headers, checksums,
  settings, and the system proxy formats of each desktop.
- `tests/e2e.rs` runs against a real core on free ports. It serves a subscription and runs two local proxy
  nodes and an origin, then drives the service as the app does:
  - add the profile and start the core;
  - pick a node and test delays;
  - send traffic through the core and check it in the connections and traffic streams;
  - check rules, logs and the DNS tool;
  - switch modes;
  - try a bad override, which is refused;
  - kill the core, which comes back with its selection intact;
  - stop.
- `tun_in_a_network_namespace` turns TUN on and off for real, under `unshare -rn`. That gives a user and
  network namespace with uid 0 inside, so no root is needed and the host's network is not touched.
- `tests/app.test.tsx` runs headless under `nsh test` with the real core and `scripts/fixture.py`. It covers
  the first run, adding a subscription, proxies and delay tests, modes, rules, logs, DNS, settings
  (Apply/Revert) and dark mode.

## Architecture

```
src/main.rs          the launcher: window, tray, single instance, signals, guardian process
src/lib.rs           the NSH root component (App) and the sidebar shell
src/pages/*.rs       Overview, Proxies, Profiles, Connections, Rules, Logs, DNS, Settings
src/ui/              the small design system: tokens, rows, groups, segmented controls, icons
crates/kairo-core    everything that is not UI:
  service.rs           the state (a tokio watch channel the UI subscribes to) and every action
  supervisor.rs        starts, watches and stops the core; restarts it after a crash
  controller.rs        mihomo's REST API and its streams (/traffic, /memory, /connections, /logs)
  config.rs            the running config: profile + override + Kairo's settings; validation by the core
  profile.rs           subscriptions and files, usage headers, update schedule
  core_bin.rs          release lookup, download, SHA-256 check, install, update
  sysproxy.rs          system proxy on macOS, GNOME and KDE: set, restore, recover
  tun.rs               TUN privilege: status and the one-time grant
  autostart.rs         launch at login (XDG autostart, LaunchAgent)
crates/kairo-tray    the StatusNotifierItem (ksni) on Linux, NSStatusItem on macOS
```

The UI never blocks: actions run on the service's tokio runtime, and the pages render from its state through
NSH signals. Long lists (connections, rules, logs) are virtual lists keyed by id.

## TUN and privileges

TUN needs the right to create a network device and change routes. Kairo gives that right to the core binary
alone, once:

- **Linux**: `pkexec setcap cap_net_admin,cap_net_bind_service=+ep <core>`. Polkit asks for the password once,
  and the core can then run TUN as your user. A core update replaces the binary, which drops the capability,
  so Kairo asks again. If Kairo runs as root, or as uid 0 inside a user namespace, nothing is needed.
- **macOS**: the core is made setuid root through the system's administrator prompt.

Releases are built by GitHub Actions (`.github/workflows/release.yml`) when a `vX.Y.Z` tag is pushed. The
notes come from `docs/releases/vX.Y.Z.md`.

## Licence

Kairo is free software under the [GNU General Public License v3.0 or later](LICENSE).

Kairo does not include mihomo. On first run, and when you ask for an update, it downloads the official build from
[MetaCubeX/mihomo's releases](https://github.com/MetaCubeX/mihomo/releases) and verifies the published checksum.
mihomo is licensed under the GPL-3.0. You can also point Kairo at a core you built yourself (Settings → Core).
