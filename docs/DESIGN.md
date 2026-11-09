# Kairo design notes

## Information architecture

```
Sidebar ─ Overview      state of the core; routing (mode, system proxy, TUN); traffic graph; profile and proxy
        ─ Proxies       groups (left) │ the selected group's nodes (right); sort Default / Delay / Name; filter
        ─ Profiles      list (usage, expiry, schedule, errors) → detail (details form, overrides editor)
        ─ Connections   live table, sortable, searchable; inspector on the right
        ─ Rules         rules in order, searchable; rule providers
        ─ Logs          live, level, search, pause, clear
        ─ DNS           lookup through the core's resolver; fake-IP flush
        ─ Settings      General, Proxy, TUN, Controller, Delay Test, Core, About
Sidebar foot ─ core state, mode, ↑ ↓ speeds, Restart to Update when one is ready
Toolbar ─ the page's title, a subtitle (counts, the group shown) and its tools
Tray    ─ status + speeds, mode, system proxy, TUN, quick switch, start/stop, show, quit
```

The first three pages are what you use every day. The next four are for looking into what is happening, and
Settings is last. The counts in the sidebar show profiles and live connections.

## The window is the platform's

Kairo draws only the content. Everything around it is the platform's own, built by the launcher around the
NSH view (`src/mac_chrome.rs`, `src/gtk_chrome.rs`), and the two sides talk through `src/chrome.rs`:

- **macOS** (built with the macOS 26 SDK, so AppKit draws Liquid Glass): an `NSSplitViewController` whose
  first item is a sidebar item, a source list of the pages with SF Symbols, the core's state at its foot,
  and a unified `NSToolbar`. The content item adjusts its safe area, so the glass sidebar and toolbar float
  over the content; the content runs under them and leaves their room (`chrome::Insets`). The page's title
  and subtitle are the window's. Switches and segmented controls are AppKit's (`NSSwitch`,
  `NSSegmentedControl`, native components). The content's colors are AppKit's, resolved for light and dark
  (increased contrast too), with the user's accent.
- **Linux**: libadwaita's `AdwNavigationSplitView`, header bars whose title and subtitle are the page's, a
  `.navigation-sidebar` list with the icon theme's symbolic icons (Adwaita on GNOME, Breeze on KDE) and the
  core's state at its foot. The content takes its colors from the GTK theme's named colors (libadwaita's;
  KDE's color schemes write them too), and its type size from the desktop's UI font.
- **A page declares its toolbar once** (`ui::toolbar`): search fields, buttons with a glyph for each
  platform (SF Symbols, freedesktop names) and pull-down choices. The chrome shows them as its own (an
  `NSSearchToolbarItem`, toolbar items, an `NSMenuToolbarItem`; a `GtkSearchEntry`, flat buttons, a menu
  button) and sends what the user does back. Without the chrome (the headless tests) NSH draws them.

## Visual rules

- **One accent, the user's**: it marks the selection, primary actions, switches and the download line of the
  graph. Everything else is the platform's neutral colors. Green, amber and red mean status only: running,
  slow, failed or REJECT. They are never used for decoration.
- **Forms like the system's own**: settings and state are grouped rows (System Settings on macOS, boxed
  lists on GNOME), a label above each group, at least 48 px per row, content centred at a readable width.
- **Dense where it is a table**: 30 px rows, tabular numbers for anything that changes (speeds, delays,
  sizes); column widths follow the text size.
- **Light and dark** follow the system live, or a fixed choice in Settings (the window's appearance on macOS,
  libadwaita's color scheme on Linux).
- **Accessibility**: AppKit and libadwaita handle Reduce Transparency and Increase Contrast for the chrome;
  the content uses the increased-contrast colors (macOS, read at launch) and stronger borders and secondary
  text under GNOME's high contrast.
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
