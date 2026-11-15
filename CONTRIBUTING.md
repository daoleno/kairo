# Contributing

Issues and pull requests are welcome.

- **Build**: `cargo run` needs Rust stable, plus GTK 4 and WebKitGTK 6.0 development packages on Linux
  (`libgtk-4-dev libadwaita-1-dev libwebkitgtk-6.0-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev` on Debian/Ubuntu).
- **Before a pull request**: run `cargo check --workspace` and `cargo test --workspace`.
  The e2e tests need a core: `scripts/fetch-mihomo.sh` (or `KAIRO_TEST_CORE=/path/to/mihomo`).
  With [nsh](https://nsh.dev) installed, `scripts/ui-test.sh` also runs the headless UI tests (CI runs them too).
  It runs them in a user + network namespace of their own (`unshare -rn`), in a scratch `KAIRO_HOME`.
  A bare `nsh test` is safe too, by construction: only `kairo` and `kairo-server` may use the real home
  (`kairo_core::paths`). Any other process (the nsh test runner, a test binary) gets a scratch home of its own,
  no system proxy and no TUN, and stops with a panic if `KAIRO_HOME` is the real home.
- **Develop next to a proxy you rely on**: set `KAIRO_HOME` to a scratch directory and `KAIRO_NO_SYSPROXY=1`,
  and pick ports in Settings that your running proxy does not use.
- **Style**: follow the code around you. The UI rules (one accent, dense rows, native frame) are in
  [docs/DESIGN.md](docs/DESIGN.md). Product text is short, plain English, and a row has no second line
  that only explains itself.
- **Licence**: contributions are under the GPL-3.0-or-later, like the rest of Kairo.
