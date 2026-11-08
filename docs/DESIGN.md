# Kairo design notes

## Information architecture

```
Sidebar ─ Overview      state of the core, mode, system proxy, TUN, traffic graph, profile and proxy cards
        ─ Proxies       groups (left) │ the selected group's nodes (right); sort Default / Delay / Name; filter
        ─ Profiles      list (usage, expiry, schedule, errors) → detail (details form, overrides editor)
        ─ Connections   live table, sortable, searchable; inspector on the right
        ─ Rules         rules in order, searchable; rule providers
        ─ Logs          live, level, search, pause, clear
        ─ DNS           lookup through the core's resolver; fake-IP flush
        ─ Settings      General, Proxy, TUN, Controller, Delay Test, Core, About
Footer  ─ core state, mode, ↑ ↓ speeds (always visible)
Tray    ─ status + speeds, mode, system proxy, TUN, quick switch, start/stop, show, quit
```

The first three pages are what you use every day. The next four are for looking into what is happening, and
Settings is last. The counts in the sidebar show profiles and live connections.

## Visual rules

- **One accent**: system blue (`#007aff` in light, `#0a84ff` in dark) marks the selection, primary actions,
  switches and the download line of the graph. Everything else uses neutral greys from the theme tokens
  (`theme.json`). Green, amber and red mean status only: running, slow, failed or REJECT. They are never used
  for decoration.
- **Dense, not cramped**: 30 px table rows, 13 px body text, 11–12 px secondary text and tabular numbers for
  anything that changes (speeds, delays, sizes). Groups are cards with hairline dividers, like the system's
  own settings.
- **Native frame**: the window, title bar, menus (popup menus are the toolkit's own), file dialogs and the
  tray come from the platform. Kairo draws only the content.
- **Light and dark** follow the system live (the XDG portal's `color-scheme` on Linux and NSAppearance on
  macOS), or a fixed choice in Settings.
- **Quiet motion**: no animation besides what the toolkit does. Live data updates in place.

## Behaviour rules

- **Nothing blocks**: every action runs on the service runtime, and buttons show progress ("Testing…",
  "Updating…").
- **Validate before applying**: the core checks every profile, override and settings change before Kairo
  uses it. A failure leaves the running config as it was and shows the core's own words.
- **The network comes back**: the system proxy is restored on every exit path. That covers a clean quit,
  signals, a panic, and a crash or SIGKILL, through the guardian process and recovery on the next start.
- **Settings use explicit Apply** for typed fields (ports, secret, URLs): one Apply validates and restarts the
  core once. Switches and segmented controls apply at once.
