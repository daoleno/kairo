# Kairo design notes

## Information architecture

```
Home           the switch and what it means now ("Connected · Tokyo 01 · 22 ms"); AI: a tile per service
               (the country it sees, what tools use it), what stays on this computer and what goes direct;
               Location, AI exit, Route, Apps & Sites; the two speeds; the subscription's usage. Without a
               subscription: Welcome, one link field and Connect.
Activity       which tools talk to which AI services: tools │ the AI exit │ services, lines as thick as the
               traffic, dashed for what stays off the exit; "All connections" opens the table
Apps & Sites   the subscription's other selectors (Netflix, Telegram …), one choice each (AI is on Home)
── Advanced ── (the sidebar folds away on Home and Apps & Sites)
Groups         groups (left) │ the selected group's members (right); sort, filter, delay tests
Subscriptions  list (usage, expiry, schedule, errors) → detail (details form, overrides editor)
Connections    live table, sortable, searchable; inspector; totals and core memory
Rules · Logs
Settings       General, AI, Proxy (system proxy), TUN Mode, About; Advanced (folded): Network, TUN Stack,
               Delay Test, Core
⌘K / Ctrl+K    every page (DNS is only here), action and location
⌘1…⌘8          Home, Activity, Apps & Sites, Groups, Subscriptions, Connections, Rules, Logs
Menu bar       Connection (Connect, Location, Route, Core) and Go (macOS); the main menu (Linux)
Tray           status, Connected, Location (Fastest and the 15 quickest), Route, Open Kairo, Quit
```

Everyday use needs Home only. A location is a member of the main group (the selector most other groups
follow), flattened through member selector groups and grouped by region. The provider's info entries
(usage, expiry) are notes, never places.

## The window is the platform's

Kairo draws only the content. Everything around it is the platform's own, built by the launcher around the
NSH view (`src/mac_chrome.rs`, `src/gtk_chrome.rs`), and the two sides talk through `src/chrome.rs`:

- **macOS** (built with the macOS 26 SDK, so AppKit draws Liquid Glass): an `NSSplitViewController` whose
  first item is a sidebar item, a source list of the pages with SF Symbols, the core's state at its foot,
  and a unified `NSToolbar`. The content item adjusts its safe area, so the glass sidebar and toolbar float
  over the content; the content runs under them and leaves their room (`chrome::Insets`). The page's title
  and subtitle are the window's. Switches and segmented controls are AppKit's (`NSSwitch`,
  `NSSegmentedControl`, native components). The content's colors are AppKit's, resolved for light and dark
  (increased contrast too), with Kairo's accent, which an `AccentColor` asset also gives AppKit's controls.
- **Linux**: libadwaita's `AdwOverlaySplitView` (the sidebar shows for Advanced pages and folds away on Home), header bars whose title and subtitle are the page's, a
  `.navigation-sidebar` list with the icon theme's symbolic icons (Adwaita on GNOME, Breeze on KDE) and the
  core's state at its foot, and a main menu (Search Kairo…, Restart Core, Settings, Quit) with the shortcuts as accelerators, since keys land outside NSH's view there. The content takes its colors from the GTK theme's named colors (libadwaita's;
  KDE's color schemes write them too), and its type size from the desktop's UI font.
- **A page declares its toolbar once** (`ui::toolbar`): search fields, buttons with a glyph for each
  platform (SF Symbols, freedesktop names) and pull-down choices. The chrome shows them as its own (an
  `NSSearchToolbarItem`, toolbar items, an `NSMenuToolbarItem`; a `GtkSearchEntry`, flat buttons, a menu
  button) and sends what the user does back. Without the chrome (the headless tests) NSH draws them.

## Visual rules

- **One accent, Ember** (`src/accent.rs`; never the platform's blue): it marks the switch when connected,
  the place in use, primary actions, focus and the traffic line. Everything else is the platform's neutral colors. Green, amber and red mean status only: running,
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
- **Round**: capsules for buttons, segmented controls and pop-up buttons; grouped rows at 20 px radius, popovers
  at 22, concentric inside them.
- **Quiet motion**: no animation besides what the toolkit does. Live data updates in place.

## AI

- **One exit, pinned**: AI traffic goes to a select group (the subscription's own AI group when its first
  rule for OpenAI or Anthropic points at one, else a hidden `Kairo AI`). The exit is a server, kept by its
  type, host and port, so a rename does not lose it. Choosing it goes straight to the core's `select`: no
  connection is closed. A server that leaves the subscription raises an alert; AI follows the Location
  until a new one is pinned. An AI group that already went somewhere stays there: that is the pin.
- **Rules first, then the profile's**: the layer's rules come before the profile's (loopback and private
  addresses direct; domestic AI direct; downloads to the Location; AI hosts to the AI group); a user's
  override `prepend-rules` still come before these. "All Traffic" is rules with the Location as the rest,
  not mihomo's global mode (which ignores rules).
- **Say what was measured**: reachable (a TCP/HTTP answer through the exit), API (the status without a key,
  as the service gives it), country (Cloudflare's trace when the service is behind it, else an estimate marked
  as one), a connection held 90 seconds. No "fine", no "safe": the footer on every detail says Kairo shows
  what the service sees and that the person's location and the service's terms still apply.
- **Counts, not content**: the Activity page keeps (tool, service, bytes, open connections); never a URL, a
  header or a key. Kairo's own checks are left out. The core reads process names (`find-process-mode`) for it.
- **Never**: change the system language or time zone, give an IP to an account, or read AI traffic.

## Behaviour rules

- **Nothing blocks**: every action runs on the service runtime, and buttons show progress ("Testing…",
  "Updating…").
- **Validate before applying**: the core checks every profile, override and settings change before Kairo
  uses it. A failure leaves the running config as it was and shows the core's own words.
- **The network comes back**: the system proxy is restored on every exit path. That covers a clean quit,
  signals, a panic, and a crash or SIGKILL, through the guardian process and recovery on the next start.
- **Settings use explicit Apply** for typed fields (ports, secret, URLs): one Apply validates and restarts the
  core once. Switches and segmented controls apply at once.
- **Plain words first**: the state is Not Connected, Connecting, Connected or Couldn't Connect. An error says
  what happened and has one button that fixes it (Use a Free Port, Allow TUN…, Try Again). A provider's
  notice is shown as "Your provider says: …".
- **Automatic, until you choose**: a new subscription starts on the fastest place, and Kairo checks again
  every ten minutes. It moves only when the current place fails or is much slower (> 1.5× + 40 ms). Picking a
  place by hand, here or in Groups, ends Automatic. Settings from 0.1 keep the node picked there.
- **The last good config stays**: an update is validated before it replaces anything. A failed one is
  retried hourly, and Home says it is using the saved copy.
