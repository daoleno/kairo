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
| Subscription links (they carry the account's token), node passwords | `profiles.json`, `profiles/*.yaml`, `runtime/config.yaml`: 0600 in a 0700 folder |
| The controller's secret | `settings.json`, 0600. Never sent to a page. |
| What was visited (Activity, Statistics) | `<home>`, 0600 |
| The machine's network | TUN, the system proxy, the core's ports |

## Threats, and what stops each

1. **Someone on the network guesses the password.**
   - There is no default password. The first one is set with a one-time setup code that the server prints at
     start (`kairo-server status` shows it again), or with `kairo-server password` on the machine.
   - Logins are limited. Each address gets 5 free tries, then waits 30 s, doubling up to 15 min. Across all
     addresses, 30 tries a minute.
   - argon2 checks one password at a time, so a flood cannot exhaust memory.
   - *Tests:* `first_run_needs_the_setup_code_and_no_password_is_set_by_default`,
     `wrong_passwords_are_slowed_down_even_before_the_right_one`; unit tests in `auth.rs`.
2. **Another website the person visits drives the server** (CSRF, cross-site WebSocket hijacking).
   - Every request that changes something, and the WebSocket upgrade, must carry an `Origin` equal to the
     server's own. Requests with no Origin are refused too.
   - The cookie is `SameSite=Strict`.
   - *Test:* `another_sites_page_can_not_sign_in_or_open_the_socket`.
3. **DNS rebinding** (a site's own name, re-pointed at the server's address).
   - The `Host` must be one of: an IP address, `localhost` or `*.localhost`, the machine's name (or
     `name.local`), or a name given with `--host` / `--public-url`. Anything else gets 421.
   - *Test:* `another_name_for_this_address_is_refused`.
4. **The session token is stolen from a page or a log.**
   - The cookie is `HttpOnly`, `Path=/` and has no `Domain`. Behind HTTPS it is `__Host-` and `Secure`.
   - The token is never in a URL or a response body, and never in a server log.
   - The page sends a strict CSP: scripts only by file or by hash, `frame-ancestors 'none'`,
     `connect-src 'self'`. Clickjacking is blocked by `X-Frame-Options: DENY`.
   - *Tests:* `the_session_cookie_is_http_only_strict_and_this_hosts_alone`,
     `behind_https_the_cookie_is_secure_and_host_prefixed`,
     `nothing_opens_without_a_session_and_no_secret_travels_in_a_url`,
     `the_page_is_sent_with_its_protections`.
5. **A signed-in page learns more than it shows.**
   - Every message to a page passes through one redaction step (`redact.rs`). Subscription links become
     `https://host/`, and the controller's secret becomes a placeholder.
   - Every string of every message is scrubbed of the links and the secret. That covers an error, a log line or
     a notice that quotes them, percent-encoded copies, and links a page sent that never became a subscription.
   - Placeholders a page sends back keep the real value.
   - *Tests:* `a_subscription_link_and_the_controller_secret_never_reach_a_page`; unit tests in `redact.rs`.
6. **A signed-in page runs a program of its choosing.**
   - A page cannot change `core_path`, the program the server starts.
   - It also cannot change the desktop app's login item, minimizing or tray.
   - Uploaded configs land under `<home>/web/imports` with a sanitized name, and are deleted after import.
   - Pictures are served by file name from Kairo's icon folder only, and only to a session.
   - *Tests:* `a_page_can_not_pick_the_program_the_server_runs`; `browser_file_names_stay_in_the_folder`.
7. **A lost or shared device stays signed in.**
   - Sessions last 30 days and slide with use.
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
   - With that off, a subscription's own DNS listener and `listeners` are moved to loopback.
   - *Tests:* `without_the_network_the_profiles_own_listeners_answer_this_machine_only` and
     `settings_win_over_the_profile` (`config.rs`).
10. **Two Kairos on one folder.** A `flock` on `<home>/kairo.lock` stops a second server.
    - *Test:* `a_second_server_on_the_same_folder_will_not_start`.
    - The desktop app takes no lock (0.3.2 and earlier). Do not run it on a folder a server uses.

## Where to listen, and TLS

- By default the server listens on `127.0.0.1:7979` and nothing else can reach it.
- Use `http://kairo.localhost:7979` rather than `localhost`. Browsers send cookies by host, not by port, so
  another app on `localhost` would see Kairo's cookie, and Kairo would see that app's. `*.localhost` is still a
  secure context: Copy works.
- From other devices, the server does not do TLS itself. Use one of these:
  - **Tailscale:**
    1. Run `tailscale serve --bg 7979`, which gives a certificate for `machine.tailnet.ts.net`.
    2. Start `kairo-server serve --public-url https://machine.tailnet.ts.net`.
  - **An SSH tunnel:** `ssh -L 7979:127.0.0.1:7979 machine`, then open `http://kairo.localhost:7979`.
  - **A reverse proxy with a real certificate** (Caddy, nginx) on the same machine, forwarding to 127.0.0.1:7979
    with the `Host` header. Start with `--public-url https://its.name`.
- `--listen 0.0.0.0:7979` over plain HTTP works on a trusted LAN. The server warns at start: the password and
  the cookie cross the network readable, and the browser's Copy buttons do not work over plain HTTP.

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
- **Availability.** A signed-in person can stop the proxy. That is the point.
- **The plain-HTTP warning is advice, not a refusal.** Running without HTTPS beyond loopback is the operator's
  choice.

## What a reviewer should attack first

1. `redact.rs`: any message type, or any new field, that carries a link or secret in a form `Secrets` does not
   know (base64, a different encoding, a link split across log lines).
2. `http.rs`: `host_allowed` and `same_origin` (IPv6 literals, ports, `Origin: null`, trailing dots), and `under`
   (static files: encodings, symlinks).
3. `dispatch.rs`: each command, for a way to write a file or start a program (`UpdateSettings` fields, `AddYaml`
   names, `RenameProfile` URLs, `TestNode` URLs as SSRF to the machine's own services).
4. `auth.rs`: the limiter keyed by address. Behind a reverse proxy every request comes from 127.0.0.1, so the
   global limit is what holds.
