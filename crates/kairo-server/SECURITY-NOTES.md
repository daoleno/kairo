# kairo-server: security notes

`kairo-server` runs Kairo's engine on a machine and serves Kairo's UI to a browser behind a password. Anyone
who gets in controls how that machine reaches the internet. They can:
- switch it to their own subscription;
- turn Every app (TUN) on or off;
- see every connection it makes.

These notes say what it protects, how, and where each control is tested. Most controls are tested in
`tests/security.rs`, which runs the real program.

## What is protected

| Asset | Where it lives |
|---|---|
| The password | `<home>/web/password`: an argon2id hash, 0600 |
| Sessions | `<home>/web/sessions.json`: SHA-256 of each token, 0600. The token itself is only in the browser's cookie. |
| Browsers that signed in before | `<home>/web/devices.json`: SHA-256 of each device token, and the addresses that signed in, 0600. A device token signs no one in; it only keeps its browser's sign-in tries apart from everyone else's. |
| Subscription links (they carry the account's token), node passwords | `profiles.json`, `profiles/*.yaml`, `runtime/config.yaml`: 0600 in a 0700 folder |
| The controller's secret | `settings.json`, 0600. Never sent to a page. |
| What was visited (Activity, Statistics) | `<home>`, 0600 |
| The machine's network | TUN, the system proxy, the core's ports |

## Threats, and what stops each

1. **Someone on the network guesses the password.**
   - There is no default password. The first one is set with a one-time setup code. `kairo-server status` shows
     it, and a server started in a terminal prints it. It is never written to the log, nor to stdout when that is
     not a terminal (under systemd, stdout is the journal).
   - Sign-in tries are limited (`auth.rs`, `Limiter`):
     - Each trier gets 5 free wrong tries, then waits 30 s, doubling up to 15 min (about 100 tries a day).
     - A trier is an address (IPv6 by its /64), or a browser that signed in before (its `kairo_device` cookie).
     - A try counts from the moment it starts, so tries sent at once cannot pass the limit together.
     - A browser that signed in before is never held by others: not by its address, not by the shared limit.
       Neither is an address that signed in before.
     - Everyone else shares one bucket of 30 tries that refills one a minute: at most about 1,500 tries a day,
       however many addresses they come from.
   - argon2 checks one password at a time, so a flood cannot exhaust memory.
   - *Tests:* `first_run_needs_the_setup_code_and_no_password_is_set_by_default`,
     `wrong_passwords_are_slowed_down_even_before_the_right_one`,
     `tries_at_once_count_and_the_owners_browser_is_never_kept_out`; unit tests in `auth.rs`.
2. **Another website the person visits drives the server** (CSRF, cross-site WebSocket hijacking).
   - Every request that changes something, and the WebSocket upgrade, must carry an `Origin` equal to the
     server's own: scheme, host and port. The scheme is `--public-url`'s for its name, else `https` when the
     server has its own certificate, else `http`. Requests with no Origin are refused too.
   - The cookie is `SameSite=Strict`.
   - *Test:* `another_sites_page_can_not_sign_in_or_open_the_socket`.
3. **DNS rebinding** (a site's own name, re-pointed at the server's address).
   - The `Host` must be one of: an IP address, `localhost` or `*.localhost`, the machine's name (or
     `name.local`), or a name given with `--host` / `--public-url`. Anything else gets 421.
   - *Test:* `another_name_for_this_address_is_refused`.
4. **The session token is stolen from a page or a log.**
   - The cookie is `HttpOnly`, `Path=/` and has no `Domain`. Over HTTPS (its own certificate, or behind
     `--public-url https://…`) it is `__Host-` and `Secure`.
   - The token is never in a URL or a response body, and never in a server log.
   - The page sends a strict CSP: scripts only by file or by hash, `frame-ancestors 'none'`,
     `connect-src 'self'`. Clickjacking is blocked by `X-Frame-Options: DENY`.
   - *Tests:* `the_session_cookie_is_http_only_strict_and_this_hosts_alone`,
     `behind_https_the_cookie_is_secure_and_host_prefixed`,
     `with_a_certificate_every_page_is_https_and_its_cookie_secure`,
     `nothing_opens_without_a_session_and_no_secret_travels_in_a_url`,
     `the_page_is_sent_with_its_protections`.
5. **A signed-in page learns more than it shows.**
   - Every message to a page passes through one redaction step (`redact.rs`). Subscription links become
     `https://host/`, and the controller's secret becomes a placeholder.
   - Every string of every message is scrubbed of the links and the secret. That covers an error, a log line or
     a notice that quotes them, percent-encoded copies, and links a page sent that never became a subscription.
   - The links of every config's `proxy-providers` and `rule-providers` (the running one, every subscription,
     every override) are scrubbed the same way. They are read again when a file changes.
   - Placeholders a page sends back keep the real value.
   - *Tests:* `a_subscription_link_and_the_controller_secret_never_reach_a_page` (the core really logs both
     links); unit tests in `redact.rs`.
6. **A signed-in page runs a program of its choosing, or names a file.**
   - A page cannot change `core_path`, the program the server starts.
   - A page cannot choose the controller's secret. It keeps the one it has; a new one is made by the server
     (`new_secret`), and the page only learns that it changed.
   - A subscription is named by its id. A command naming one that does not exist is refused before anything
     touches a file, and the files of one are named only by an id Kairo made (12 hex digits; `paths.rs`).
   - It also cannot change the desktop app's login item, minimizing or tray.
   - Uploaded configs land under `<home>/web/imports` with a sanitized name, and are deleted after import.
   - Pictures are served by file name from Kairo's icon folder only, and only to a session.
   - *Tests:* `a_page_can_not_pick_the_program_the_server_runs`, `a_page_can_not_choose_the_controller_secret`,
     `a_page_names_only_subscriptions_that_exist`; `browser_file_names_stay_in_the_folder`,
     `only_an_id_kairo_made_names_a_file`.
7. **A lost or shared device stays signed in.**
   - Sessions last 30 days and slide with use. An open socket of a session past its 30 days closes too.
   - Settings › Signed-in browsers lists them and signs out any one, or all but this one. A signed-out browser's
     open socket closes at once (code 4401).
   - `kairo-server password` sets a new password and ends every session.
   - Sessions survive a restart.
   - *Test:* `a_revoked_browser_is_signed_out_at_once_and_a_restart_keeps_the_rest`.
8. **Another account on the machine reads the secrets.**
   - Kairo's folder is 0700 and every file in it is 0600. Files an older Kairo left readable are fixed at start.
   - The CLI's control socket (`<home>/web/control.sock`, 0600) is the only way in without a password. Whoever
     can open it is already the owner.
   - *Tests:* `the_password_and_sessions_are_its_owners_alone`; `what_kairo_writes_is_its_owners_alone`.
9. **The core's own ports are reached from the network.**
   - The controller always listens on 127.0.0.1. The server never forwards to it; pages speak Kairo's own
     protocol.
   - The proxy port opens to the network only when "Allow other devices on the network" is on.
   - With that off, no inbound of a config answers another device, whether the subscription's or the override's.
     mihomo opens several of them on every address whatever `allow-lan` says (tried against 1.19.33 in a network
     namespace), so Kairo handles each:
     - `dns.listen`, `listeners` and port `tunnels` are moved to loopback.
     - `ss-config`, `vmess-config` and `tuic-server` take no listen address, so they are left out (with a log
       line). So are `tcptun-config` and `udptun-config`, which no current core reads.
   - A config's own `listeners` of type `tun` never reach the core, whatever the setting: Kairo's TUN setting is
     the only TUN.
   - *Tests:* `with_other_devices_off_no_inbound_answers_them` (`tests/lan.rs`, a real core in a namespace),
     `without_the_network_the_profiles_own_listeners_answer_this_machine_only`, `settings_win_over_the_profile`.
10. **"Allow other devices" turns the proxy into a way into this machine.** This is in `kairo-core`, so the
    desktop app has it too.
    - With other devices allowed, a connection from another device to this machine's loopback is refused:
      127/8, ::1, 0.0.0.0, `localhost` names, and names that resolve there.
    - Programs on this machine keep their loopback, and other devices proxy to everywhere else as before.
    - The guard is the first rule, so the core runs its `rule` mode while other devices are allowed. Direct is
      then "everything DIRECT", and All Traffic is "everything through GLOBAL". mihomo's own `direct` and
      `global` modes, and an inbound's own `proxy`, would skip every rule:
      - a listener on the network with a `proxy` of its own becomes an `IN-NAME` rule to the same exit;
      - a port tunnel on the network with one answers this machine only.
    - The source is checked first, so a local program's connection is never resolved for the guard.
    - Another device's connection to a name is resolved here to check it. In All Traffic that is one lookup on
      this machine for each name another device proxies to.
    - A program on this machine that uses the proxy by the machine's network address, not 127.0.0.1, counts as
      another device.
    - A listener's `rule:` runs a list of `sub-rules` in place of the rules, so each such list starts with the
      guard too.
    - With TUN on as well, a local program's connection that TUN captured comes from TUN's address. If its name
      resolves to loopback (only through the core's fake-IP DNS), it is refused.
    - *Test:* `other_devices_never_reach_this_machines_loopback` (`tests/lan.rs`: rule, Direct and All Traffic;
      a listener with its own proxy, one with its own rules; a port tunnel).
11. **A signed-in page reads or maps services on this machine or its network** through the server's own
    fetches.
    - A subscription link is fetched with every redirect checked. Each hop must be http or https, and is
      checked for an address of this machine or its network (loopback, private, link-local, unique-local, CGNAT).
      The connection is held to the addresses checked, and the address it reached is checked again.
    - While TUN is on, the machine's DNS answers every name with a fake address (198.18/15). Those are looked up
      in the core's own DNS, and the real addresses are checked. The connection still goes to the fake address,
      so the core routes it by name as before. With no real answer, the name counts as this machine's. The core
      resolves the name again when it connects, so a name whose answer changes in between can still slip
      through.
    - Such an address can still be a subscription. But anything wrong with it (no answer, a status, a page that
      is not a config) is one error that says nothing of what was there.
    - A subscription is read to 16 MB at most, streamed.
    - The delay test of a node from a page uses the saved test address only. A page may save one only if it is
      http or https to a host whose every address is public. The address is checked when it is saved; the core
      resolves it again at each test. A group's own test address (from its config) is the config's: see
      "The core's own fetches" below.
    - *Tests:* `a_page_can_not_read_or_map_this_machines_services`, `a_page_can_not_point_the_delay_test_at_this_machine`,
      `a_subscription_that_leads_to_this_machine_tells_nothing_of_it` (`tests/lan.rs`: a public address's
      redirect, 17 MB with and without its length).
12. **Two Kairos on one folder.** A `flock` on `<home>/kairo.lock` stops a second server.
    - *Test:* `a_second_server_on_the_same_folder_will_not_start`.
    - The desktop app takes no lock (0.3.2 and earlier). Do not run it on a folder a server uses.
13. **The password and the session are read on the network** (plain HTTP).
    - A `--listen` address beyond loopback opens only with HTTPS (`--tls-cert` and `--tls-key`), or with
      `--insecure-http`, which says the operator accepts plain HTTP there. Otherwise that address is not opened, and
      the server says why at start, in the log and in `kairo-server status`. Its other addresses still open.
    - With a certificate every address is HTTPS (loopback too), with `Strict-Transport-Security` and the `__Host-`,
      `Secure` cookie. The certificate and key are PEM files, read again when they change (a renewal needs no
      restart). A key that is not the certificate's stops the start with that reason. There is no self-signed
      certificate made by Kairo.
    - A page on plain HTTP from another device shows one mark on its sign-in page: "Not encrypted".
    - *Tests:* `plain_http_beyond_loopback_opens_only_when_asked`,
      `with_a_certificate_every_page_is_https_and_its_cookie_secure`,
      `a_certificate_that_is_not_the_keys_stops_the_start_with_a_reason`; `scripts/web-e2e.sh` (the mark, from
      another address and not on loopback).
14. **Someone who has not signed in exhausts the server.**
    - At most 512 connections per address it listens on. A connection must send its request's headers within
      20 s, and a TLS handshake must end within 10 s (at most 64 at once).
    - A client that takes no byte of what it is sent for 60 s is let go (a write waiting on it fails), so a
      reader that stops reading gives back its connection and its file.
    - Page files are streamed from disk, never read whole: at most 64 at once (more get 503 and `Retry-After`),
      and each within 10 minutes. Sign-in does not wait on them.
    - This bounds what one client holds, not how many come: many addresses opening connections again and again
      can still keep the page from loading (see Availability).
    - *Tests:* `before_sign_in_the_page_files_stream_and_hold_no_one_up_for_ever`,
      `with_a_certificate_every_page_is_https_and_its_cookie_secure` (64 silent handshakes). The 512 cap itself
      has no test.
15. **A signed-in page fills the memory or the disk.**
    - A config is counted before it is parsed, with its aliases unfolded: more than 5,000,000 values is refused
      (a few aliases of a big anchor would otherwise unfold to gigabytes). Real subscriptions have a few hundred
      thousand.
    - Kairo keeps at most 100 subscriptions.
    - A subscription is read to 16 MB at most (item 11); an uploaded or pasted config to 8 MB.
    - *Tests:* `a_page_can_not_add_subscriptions_without_end`;
      `a_yaml_whose_aliases_unfold_too_far_is_refused_without_unfolding_it` (`config.rs`).
16. **Updates.** The update manifest is signed (ed25519) and checked before anything is downloaded.
    - It carries an expiry (a year from signing). An expired manifest offers no update, so an old signed one cannot
      be replayed to push a version since withdrawn.
    - The server's own tarball replaces the core, the docs, the page and the program. A failure part way puts every
      replaced file back, so the old server never runs with a new core.
    - A program file replaced while it ran (`/proc/self/exe` ends in ` (deleted)`) is still recognized as the
      server, so the update is the server's package, not the desktop app's.
    - *Tests:* `an_old_signed_manifest_offers_nothing`, `the_servers_tarball_replaces_the_server_and_its_page`
      (`update.rs`).

## Where to listen, and TLS

- By default the server listens on `127.0.0.1:7979` and nothing else can reach it.
- Use `http://kairo.localhost:7979` rather than `localhost`. Browsers send cookies by host, not by port, so
  another app on `localhost` would see Kairo's cookie, and Kairo would see that app's. `*.localhost` is still a
  secure context: Copy works.
- From other devices, use HTTPS. Any one of these:
  - **Its own certificate:** `kairo-server serve --listen 0.0.0.0:7979 --tls-cert cert.pem --tls-key key.pem`,
    for a name the browsers trust (your CA, or a public name's certificate). Open `https://that.name:7979`.
  - **Tailscale:**
    1. Run `tailscale serve --bg 7979`, which gives a certificate for `machine.tailnet.ts.net`.
    2. Start `kairo-server serve --public-url https://machine.tailnet.ts.net`.
  - **An SSH tunnel:** `ssh -L 7979:127.0.0.1:7979 machine`, then open `http://kairo.localhost:7979`.
  - **A reverse proxy with a real certificate** (Caddy, nginx) on the same machine, forwarding to 127.0.0.1:7979
    with the `Host` header. Start with `--public-url https://its.name`.
- A `--listen` address that cannot be had yet never stops the server, its other addresses or the core. On Linux an
  address the machine does not have yet (DHCP at boot) is bound anyway (`IP_FREEBIND`) and answers once it comes
  up. Anything else (a port in use) is tried again every 10 s. The user unit therefore does not wait for the
  network (a user unit cannot order itself after `network-online.target`).
- Plain HTTP beyond loopback needs `--insecure-http` (item 13): `--listen 0.0.0.0:7979 --insecure-http` on a
  LAN you trust. The password and the cookie then cross the network readable. The server says so at start, and
  the sign-in page shows "Not encrypted". Copy works there too.

## Not covered

- **A compromised browser or device.** It holds a valid session. Revoke it from another browser, or run
  `kairo-server password`.
- **Root on the machine.** Root can read everything. Secrets are protected by file modes, not encrypted at rest:
  a headless machine has no keyring to unlock.
- **Every app (TUN) needs a file capability on the core**
  (`sudo setcap cap_net_admin,cap_net_bind_service=+ep <core>`). The page shows the exact line; it never asks for
  an administrator's password.
  - An update of the core drops that capability. Kairo then turns Every app off and says so, and the line has to
    be run again.
  - The systemd unit must not set `NoNewPrivileges=`, or the capability is ignored.
- **Availability.** A signed-in person can stop the proxy. That is the point. Before sign-in, the limits of
  item 14 keep the server's memory and files bounded, but someone who keeps opening connections from many
  addresses can keep the page from loading for others while they do. The proxy itself is not affected.
- **The core's own fetches (open).** Item 11 covers the server's own fetches only. The core fetches what a config
  names by itself: `proxy-providers`, `rule-providers`, and the health checks of `url-test`/`fallback` groups.
  A signed-in page can put such a config in place (an override, an imported file, a subscription from a server it
  controls).
  - Pointed at an address on this machine or its network, these show whether something answers there, as a
    provider's error or a delay.
  - A `rule-provider` in text format shows more: the core logs every line it cannot parse, at warning level, and
    the page's Logs view shows that line. Tried against mihomo 1.19.33 in a namespace:
    `skip invalid domain from rule provider: invalid domain "<html><body>Internal dashboard: …"` and
    `parse classical rule [LINE-ONE-… is here] error`.
  - A stolen session can therefore read a local text or HTML service line by line through a config. Closing this
    needs the server to refuse configs whose providers point at this machine or its network, which would also
    refuse a provider served from the owner's own network. It is a decision, not yet made.
- **What the sign-in page tells before sign-in.** Anyone who reaches it learns the machine's name, and whether a
  password is set yet (the page shows the name, and the setup form or the sign-in form). A signed-in page learns
  Kairo's folder, which names the account. These stay: the page shows all three.
- **A macOS server package (blocker).** On macOS, Every app is granted by making the core setuid root
  (`chown root:admin` and `chmod u+s`: `tun.rs`, and the line `link.rs` shows). Any program of that account can
  then run the core as root with a config and folder of its own choosing (`-f`, `-d`): in effect root for every
  process of the account. Read from the code, not tried on a Mac. The desktop app has this today. A macOS
  package of the server must not ship until the grant is a launchd helper that starts only Kairo's own core with
  Kairo's own config, or no more than the network rights TUN needs.

## What a reviewer should attack first

1. `redact.rs`: any message type, or any new field, that carries a link or secret in a form `Secrets` does not
   know (base64, a different encoding, a link split across log lines).
2. `http.rs`: `host_allowed` and `same_origin` (IPv6 literals, ports, `Origin: null`, trailing dots), and `under`
   (static files: encodings, symlinks).
3. `dispatch.rs`: each command, for a way to write a file or start a program (`UpdateSettings` fields, `AddYaml`
   names, `RenameProfile` URLs, `TestNode` URLs as SSRF to the machine's own services).
4. `auth.rs`: the limiter. Behind a reverse proxy every request comes from 127.0.0.1, so all of them share one
   address: its 5 tries and waits are what holds, and the owner's browser gets in by its device cookie.
5. `config.rs`: `guard_loopback` and `keep_listeners_local`, for an inbound or a mode that skips the rules, or
   listens on the network, that they do not know.
6. `serve.rs` and `tls.rs`: what a client that has not signed in can hold open (an upgraded WebSocket gives its
   connection place back; it needs a session to upgrade), and `Options::refused` for an address it misjudges.
