//! The HTTP side: the page's files, signing in, the WebSocket, pictures.
//!
//! Every request is checked first ([`guard`]):
//! - its `Host` must be one of this server's names (a raw IP address,
//!   `localhost` and `*.localhost`, the machine's name, `--host` names and
//!   `--public-url`'s): a page of another site that a DNS trick points here
//!   is refused (DNS rebinding);
//! - a request that changes something (POST) or opens the WebSocket must
//!   come from this server's own page: its `Origin` is this `Host`
//!   (cross-site requests and cross-site WebSocket hijacking are refused);
//! - answers carry a content security policy, no framing, no referrer.

use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, Path as UrlPath, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::json;

use crate::Ctx;
use crate::auth::{AuthError, SessionKey};

pub fn router(ctx: Arc<Ctx>) -> Router {
    Router::new()
        .route("/api/session", get(session))
        .route("/api/setup", post(setup))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/ws", get(ws))
        .route("/icons/{kind}/{file}", get(icon))
        .fallback(get(file))
        .layer(middleware::from_fn_with_state(ctx.clone(), guard))
        .with_state(ctx)
}

// --- Checks ------------------------------------------------------------------------------

/// `host[:port]` → its host, lower case, without brackets.
fn host_part(authority: &str) -> String {
    let a = authority.trim().to_ascii_lowercase();
    if let Some(rest) = a.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default().to_owned();
    }
    match a.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => h.to_owned(),
        _ => a,
    }
}

/// Whether `authority` (a `Host` header) names this server.
pub fn host_allowed(ctx: &Ctx, authority: &str) -> bool {
    let host = host_part(authority);
    if host.is_empty() {
        return false;
    }
    if host.parse::<IpAddr>().is_ok() || host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    let machine = crate::machine_name().to_ascii_lowercase();
    if !machine.is_empty() && (host == machine || host == format!("{machine}.local")) {
        return true;
    }
    ctx.opts.hosts.iter().any(|h| h.eq_ignore_ascii_case(&host))
}

/// The scheme a page of this server has when reached as `host`: the public
/// URL's when that is the name (a proxy in front speaks HTTPS), else the
/// listeners' own (`tls`: HTTPS on every one).
pub fn page_scheme<'a>(public_url: Option<&'a url::Url>, tls: bool, host: &str) -> &'a str {
    match public_url {
        Some(u) if u.host_str().is_some_and(|h| h.eq_ignore_ascii_case(&host_part(host))) => u.scheme(),
        _ if tls => "https",
        _ => "http",
    }
}

/// Whether `origin` (an `Origin` header) is exactly this server's own page,
/// reached as `host` (the `Host` header) over `scheme`: the scheme, the name
/// and the port must all be the page's.
pub fn same_origin(origin: &str, scheme: &str, host: &str) -> bool {
    let Ok(o) = url::Url::parse(origin) else {
        return false;
    };
    let Ok(expected) = url::Url::parse(&format!("{scheme}://{}", host.trim())) else {
        return false;
    };
    // Exactly an origin: no path, user or query smuggled in.
    if o.path() != "/" || o.query().is_some() || !o.username().is_empty() || o.password().is_some() {
        return false;
    }
    o.scheme() == expected.scheme()
        && o.host_str().map(str::to_ascii_lowercase) == expected.host_str().map(str::to_ascii_lowercase)
        && o.port_or_known_default() == expected.port_or_known_default()
}

async fn guard(State(ctx): State<Arc<Ctx>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default().to_owned();
    if !host_allowed(&ctx, &host) {
        return problem(StatusCode::MISDIRECTED_REQUEST, "This server does not answer to that name");
    }
    let changes = request.method() != Method::GET && request.method() != Method::HEAD;
    let upgrade = headers.get(header::UPGRADE).is_some();
    if changes || upgrade {
        let origin = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()).unwrap_or_default();
        let scheme = page_scheme(ctx.opts.public_url.as_ref(), ctx.opts.tls.is_some(), &host);
        if !same_origin(origin, scheme, &host) {
            return problem(StatusCode::FORBIDDEN, "Only Kairo's own page may do that");
        }
    }
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("same-origin"));
    if ctx.opts.secure() {
        h.insert("strict-transport-security", HeaderValue::from_static("max-age=31536000"));
    }
    response
}

fn problem(status: StatusCode, text: &str) -> Response {
    let mut r = (status, axum::Json(json!({ "error": text }))).into_response();
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

// --- Sessions ----------------------------------------------------------------------------

/// The cookie's name: `__Host-` (Secure, this host only, path /) over HTTPS.
fn cookie_name(ctx: &Ctx) -> &'static str {
    if ctx.opts.secure() { "__Host-kairo_session" } else { "kairo_session" }
}

/// The device cookie: which browser signed in before (for sign-in tries
/// only; it signs no one in).
fn device_name(ctx: &Ctx) -> &'static str {
    if ctx.opts.secure() { "__Host-kairo_device" } else { "kairo_device" }
}

fn token_of(ctx: &Ctx, headers: &HeaderMap) -> Option<String> {
    cookie(headers, cookie_name(ctx))
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.trim().to_owned())
}

pub fn session_of(ctx: &Ctx, headers: &HeaderMap) -> Option<SessionKey> {
    ctx.auth.check(&token_of(ctx, headers)?)
}

fn set_cookie(ctx: &Ctx, token: &str) -> HeaderValue {
    let secure = if ctx.opts.secure() { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        cookie_name(ctx),
        crate::auth::SESSION_DAYS * 86_400
    ))
    .expect("a token is a header value")
}

fn clear_cookie(ctx: &Ctx) -> HeaderValue {
    let secure = if ctx.opts.secure() { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}",
        cookie_name(ctx)
    ))
    .expect("a header value")
}

fn agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(short_agent)
        .unwrap_or_else(|| "A browser".into())
}

/// "Firefox on Linux" from a user agent string.
pub fn short_agent(ua: &str) -> String {
    let browser = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("Firefox/") {
        "Firefox"
    } else if ua.contains("Chrome/") || ua.contains("Chromium/") {
        "Chrome"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "A browser"
    };
    let system = if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("Mac OS X") || ua.contains("Macintosh") {
        "macOS"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        ""
    };
    if system.is_empty() { browser.into() } else { format!("{browser} on {system}") }
}

async fn session(State(ctx): State<Arc<Ctx>>, headers: HeaderMap) -> Response {
    let state = if ctx.auth.needs_setup() {
        "setup"
    } else if session_of(&ctx, &headers).is_some() {
        "signed_in"
    } else {
        "sign_in"
    };
    let mut r = axum::Json(json!({ "state": state, "machine": crate::machine_name() })).into_response();
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

#[derive(Deserialize)]
struct SetupBody {
    code: String,
    password: String,
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

fn auth_error(e: AuthError) -> Response {
    let status = match e {
        AuthError::Wrong => StatusCode::UNAUTHORIZED,
        AuthError::Wait(_) => StatusCode::TOO_MANY_REQUESTS,
        AuthError::AlreadySet | AuthError::NotSet => StatusCode::CONFLICT,
        AuthError::TooShort => StatusCode::BAD_REQUEST,
        AuthError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    problem(status, &e.message())
}

fn signed_in(ctx: &Ctx, s: &crate::auth::SignedIn) -> Response {
    let mut r = axum::Json(json!({})).into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(ctx, &s.token));
    let secure = if ctx.opts.secure() { "; Secure" } else { "" };
    if let Ok(v) = HeaderValue::from_str(&format!(
        "{}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        device_name(ctx),
        s.device,
        400 * 86_400
    )) {
        r.headers_mut().append(header::SET_COOKIE, v);
    }
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

async fn setup(
    State(ctx): State<Arc<Ctx>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::Json<SetupBody>,
) -> Response {
    let device = cookie(&headers, device_name(&ctx));
    match ctx.auth.setup(&body.code, &body.password, peer.ip(), device.as_deref(), &agent(&headers)).await {
        Ok(s) => {
            log::warn!("the password was set from {}", peer.ip());
            signed_in(&ctx, &s)
        }
        Err(e) => auth_error(e),
    }
}

async fn login(
    State(ctx): State<Arc<Ctx>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::Json<LoginBody>,
) -> Response {
    let device = cookie(&headers, device_name(&ctx));
    match ctx.auth.login(&body.password, peer.ip(), device.as_deref(), &agent(&headers)).await {
        Ok(s) => signed_in(&ctx, &s),
        Err(e) => {
            if e == AuthError::Wrong {
                log::warn!("a wrong password from {}", peer.ip());
            }
            auth_error(e)
        }
    }
}

async fn logout(State(ctx): State<Arc<Ctx>>, headers: HeaderMap) -> Response {
    if let Some(key) = session_of(&ctx, &headers) {
        ctx.auth.sign_out(&key);
    }
    let mut r = axum::Json(json!({})).into_response();
    r.headers_mut().insert(header::SET_COOKIE, clear_cookie(&ctx));
    r
}

async fn ws(State(ctx): State<Arc<Ctx>>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Response {
    let Some(key) = session_of(&ctx, &headers) else {
        return problem(StatusCode::UNAUTHORIZED, "Sign in first");
    };
    upgrade
        .max_message_size(crate::dispatch::protocol_limit() + 64 * 1024)
        .on_upgrade(move |socket| crate::link::serve(ctx, key, socket))
}

// --- Files -------------------------------------------------------------------------------

/// A path of the request as a file under `root`, if it is a plain one.
fn under(root: &Path, path: &str) -> Option<PathBuf> {
    let rel = Path::new(path.trim_start_matches('/'));
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(root.join(rel))
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Static files sent at once, at most (each holds its place while it streams).
pub const MAX_TRANSFERS: usize = 64;
/// The longest one file may take to send: the page's 3 MB (brotli) at 5 KB/s.
/// This bounds a reader that reads slowly; one that stops reading is let go
/// sooner, by its connection (`serve::STALL_TIME`).
pub const TRANSFER_TIME: std::time::Duration = std::time::Duration::from_secs(600);
static TRANSFERS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(MAX_TRANSFERS)));

/// Sends `path`, or its brotli or gzip copy next to it when the browser takes one.
async fn send_file(path: &Path, headers: &HeaderMap, cache: &'static str) -> Response {
    let accepts = headers.get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok()).unwrap_or_default();
    let mut chosen = (path.to_path_buf(), None);
    for (ext, name) in [("br", "br"), ("gz", "gzip")] {
        if accepts.contains(name) {
            let p = PathBuf::from(format!("{}.{ext}", path.display()));
            if p.is_file() {
                chosen = (p, Some(name));
                break;
            }
        }
    }
    // Streamed, never read whole (the page's wasm is megabytes): so many at
    // once, each for so long; these come before anyone signs in.
    let Ok(transfer) = TRANSFERS.clone().try_acquire_owned() else {
        let mut r = problem(StatusCode::SERVICE_UNAVAILABLE, "Busy: try again in a moment");
        r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("2"));
        return r;
    };
    let Ok(file) = tokio::fs::File::open(&chosen.0).await else {
        return problem(StatusCode::NOT_FOUND, "Not found");
    };
    let len = file.metadata().await.map(|m| m.len()).ok();
    let deadline = tokio::time::sleep(TRANSFER_TIME);
    let stream = futures_util::StreamExt::take_until(tokio_util::io::ReaderStream::new(file), deadline);
    let stream = futures_util::StreamExt::map(stream, move |chunk| {
        let _held = &transfer;
        chunk
    });
    let mut r = Response::new(Body::from_stream(stream));
    let h = r.headers_mut();
    if let Some(len) = len {
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    }
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(path)));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if let Some(enc) = chosen.1 {
        h.insert(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
    }
    r
}

/// Whether a file's name carries its content's digest (`nsh.89070fe10a.js`):
/// it never changes, so browsers keep it.
fn digested(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    name.split('.').any(|part| part.len() == 10 && part.chars().all(|c| c.is_ascii_hexdigit()))
        || path.components().any(|c| {
            c.as_os_str().to_str().is_some_and(|s| s.rsplit('.').next().is_some_and(|d| d.len() == 10 && d.chars().all(|c| c.is_ascii_hexdigit())))
        })
}

async fn file(State(ctx): State<Arc<Ctx>>, request: Request) -> Response {
    let path = request.uri().path();
    if path == "/" || path == "/index.html" {
        let index = ctx.opts.web.join("index.html");
        let mut r = send_file(&index, request.headers(), "no-cache").await;
        // From the page as it is now (an update may have replaced it).
        let csp = match tokio::fs::read_to_string(&index).await {
            Ok(text) => csp_for(&text),
            Err(_) => ctx.csp.clone(),
        };
        if let Ok(csp) = HeaderValue::from_str(&csp) {
            r.headers_mut().insert("content-security-policy", csp);
        }
        return r;
    }
    let Some(p) = under(&ctx.opts.web, path).filter(|p| p.is_file()) else {
        return problem(StatusCode::NOT_FOUND, "Not found");
    };
    let cache = if digested(&p) { "public, max-age=31536000, immutable" } else { "no-cache" };
    send_file(&p, request.headers(), cache).await
}

async fn icon(
    State(ctx): State<Arc<Ctx>>,
    UrlPath((kind, name)): UrlPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if session_of(&ctx, &headers).is_none() {
        return problem(StatusCode::UNAUTHORIZED, "Sign in first");
    }
    if !matches!(kind.as_str(), "apps" | "sites")
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        || name.starts_with('.')
        || !(name.ends_with(".png") || name.ends_with(".svg"))
    {
        return problem(StatusCode::NOT_FOUND, "Not found");
    }
    let p = ctx.svc.paths().icons_dir().join(&kind).join(&name);
    let mut r = send_file(&p, &HeaderMap::new(), "private, max-age=3600").await;
    if name.ends_with(".svg") {
        // A site's SVG is shown as a picture, never run as a document.
        r.headers_mut().insert(
            "content-security-policy",
            HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
        );
    }
    r
}

/// The page's content security policy: its own files, its own WebSocket,
/// and the inline scripts of `index.html` by their digests.
pub fn csp_for(index: &str) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let mut hashes = Vec::new();
    let mut rest = index;
    while let Some(start) = rest.find("<script") {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else { break };
        let tag = &after[..open_end];
        let body_start = open_end + 1;
        let Some(close) = after[body_start..].find("</script>") else { break };
        let body = &after[body_start..body_start + close];
        if !tag.contains("src=") {
            let digest = Sha256::digest(body.as_bytes());
            hashes.push(format!("'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(digest)));
        }
        rest = &after[body_start + close..];
    }
    format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval' {}; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:; \
         frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        hashes.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert!(same_origin("http://127.0.0.1:7979", "http", "127.0.0.1:7979"));
        assert!(same_origin("http://kairo.localhost:7979", "http", "kairo.localhost:7979"));
        assert!(same_origin("https://kairo.example.ts.net", "https", "kairo.example.ts.net"));
        assert!(same_origin("http://[::1]:7979", "http", "[::1]:7979"));
        assert!(!same_origin("http://evil.example:7979", "http", "127.0.0.1:7979"));
        assert!(!same_origin("http://127.0.0.1:8080", "http", "127.0.0.1:7979"), "another port is another site");
        assert!(!same_origin("https://127.0.0.1:7979", "http", "127.0.0.1:7979"), "another scheme is another site");
        assert!(!same_origin("https://127.0.0.1", "http", "127.0.0.1:443"));
        assert!(!same_origin("http://kairo.example.ts.net", "https", "kairo.example.ts.net"));
        assert!(!same_origin("null", "http", "127.0.0.1:7979"));
        assert!(!same_origin("", "http", "127.0.0.1:7979"));
        // Behind its public URL a page is HTTPS; reached directly, plain HTTP.
        let public = url::Url::parse("https://kairo.example.ts.net").unwrap();
        assert_eq!(page_scheme(Some(&public), false, "kairo.example.ts.net"), "https");
        assert_eq!(page_scheme(Some(&public), false, "127.0.0.1:7979"), "http");
        assert_eq!(page_scheme(None, false, "127.0.0.1:7979"), "http");
        // HTTPS of its own: every page is.
        assert_eq!(page_scheme(None, true, "127.0.0.1:7979"), "https");
        assert_eq!(page_scheme(Some(&public), true, "192.168.1.2:7979"), "https");
    }

    #[test]
    fn paths_stay_in_the_folder() {
        let root = Path::new("/srv/web");
        assert_eq!(under(root, "/nsh.js"), Some(PathBuf::from("/srv/web/nsh.js")));
        assert_eq!(under(root, "/../etc/passwd"), None);
        assert_eq!(under(root, "/a/../../etc"), None);
        assert_eq!(under(root, "//etc/passwd"), Some(PathBuf::from("/srv/web/etc/passwd")));
    }

    #[test]
    fn the_policy_names_the_inline_scripts() {
        let csp = csp_for("<script>a()</script><script type=\"module\">b()</script><script src=\"x.js\"></script>");
        assert_eq!(csp.matches("'sha256-").count(), 2);
        assert!(csp.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn agents() {
        assert_eq!(short_agent("Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0"), "Firefox on Linux");
        assert_eq!(short_agent("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Version/18.0 Mobile/15E148 Safari/604.1"), "Safari on iOS");
    }
}
