//! The pictures of apps and sites: an app's own icon from the system, a site's own icon fetched
//! from the site, kept in a folder on disk.
//!
//! - **Sites**: the icon the site's page declares (`<link rel="icon">`, the touch icon) or its
//!   `/favicon.ico`, from the site itself and over the route that site uses (through the core's
//!   mixed port while the core runs, so Kairo's own rules apply to it as to the person's browser;
//!   directly when it does not). Never a favicon service of a third party.
//! - **Bounded**: a page of at most 256 KB, an icon of at most 512 KB, a few redirects, http and
//!   https only, no loopback or private addresses, ten seconds in all. What comes back is decoded
//!   with limits and written again as a small PNG (or kept as an SVG that has nothing active in
//!   it): a file that is not a picture is thrown away.
//! - **Kept**: `icons/sites/<domain>.png|svg` and `icons/apps/<name>.png`; a failure is a
//!   `.none` marker, tried again after three days; a picture is looked at again after thirty.
//!   Nothing waits for a fetch: the page shows what is on disk (or a letter) and is told when
//!   something new arrives.
//! - **Apps**: the system's own icon of the program, by platform (`apps`).

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use futures_util::StreamExt;
use url::Url;

pub mod apps;

/// The longest page read to look for the icon's address.
pub const MAX_PAGE: usize = 256 * 1024;
/// The largest icon file taken.
pub const MAX_ICON: usize = 512 * 1024;
/// How many redirects one request follows.
pub const MAX_REDIRECTS: usize = 4;
/// The side of the stored picture, in pixels (shown at 14 to 28 points).
pub const SIDE: u32 = 128;
/// How long a fetched picture is trusted, and how long a failure is remembered.
pub const KEEP: Duration = Duration::from_secs(30 * 24 * 3600);
pub const RETRY: Duration = Duration::from_secs(3 * 24 * 3600);

/// A stored picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    /// `png` or `svg`.
    pub ext: &'static str,
    pub bytes: Vec<u8>,
}

/// What the rules for a request allow.
#[derive(Debug, Clone, Copy, Default)]
pub struct Policy {
    /// Loopback and private addresses are fine (tests against a server on this machine).
    pub allow_local: bool,
    /// This one host is fine whatever it is (the server of a test), the rest keep the rules.
    pub allow_host: Option<&'static str>,
}

// --- The cache ---------------------------------------------------------------------------

/// What the folder says about a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cached {
    /// A picture, and whether it is old enough to look at again.
    Have { path: PathBuf, stale: bool },
    /// Looked for and not found, recently: do not try yet.
    Missing,
    /// Never looked for (or the failure is old): try.
    Unknown,
}

/// The folder of pictures and the names being fetched now.
pub struct Icons {
    dir: PathBuf,
    busy: Mutex<HashSet<String>>,
}

/// A name made safe for a file name: lower case, letters, digits, dot, dash, underscore.
pub fn file_key(name: &str) -> Option<String> {
    let key: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let key = key.trim_matches('.').to_owned();
    (!key.is_empty() && key.len() <= 128 && !key.contains("..")).then_some(key)
}

fn age(path: &Path, now: SystemTime) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(now.duration_since(modified).unwrap_or_default())
}

impl Icons {
    pub fn new(dir: PathBuf) -> Icons {
        Icons {
            dir,
            busy: Mutex::new(HashSet::new()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn folder(&self, kind: &str) -> PathBuf {
        self.dir.join(kind)
    }

    /// What is kept for `name` under `kind` (`sites` or `apps`).
    pub fn look(&self, kind: &str, name: &str, now: SystemTime) -> Cached {
        let Some(key) = file_key(name) else {
            return Cached::Missing;
        };
        let folder = self.folder(kind);
        for ext in ["png", "svg"] {
            let path = folder.join(format!("{key}.{ext}"));
            if let Some(a) = age(&path, now) {
                return Cached::Have {
                    path,
                    stale: a > KEEP,
                };
            }
        }
        match age(&folder.join(format!("{key}.none")), now) {
            Some(a) if a < RETRY => Cached::Missing,
            _ => Cached::Unknown,
        }
    }

    /// Writes a found picture (and clears an old failure).
    pub fn store(&self, kind: &str, name: &str, icon: &Icon) -> std::io::Result<PathBuf> {
        let key = file_key(name)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "name"))?;
        let folder = self.folder(kind);
        let path = folder.join(format!("{key}.{}", icon.ext));
        crate::paths::write_atomic(&path, &icon.bytes)?;
        // The other format and the failure marker are old news.
        let other = folder.join(format!("{key}.{}", if icon.ext == "png" { "svg" } else { "png" }));
        let _ = std::fs::remove_file(other);
        let _ = std::fs::remove_file(folder.join(format!("{key}.none")));
        Ok(path)
    }

    /// Remembers that nothing was found, so it is not asked again for a while.
    pub fn store_none(&self, kind: &str, name: &str) {
        if let Some(key) = file_key(name) {
            let _ = crate::paths::write_atomic(&self.folder(kind).join(format!("{key}.none")), b"");
        }
    }

    /// Takes the right to fetch `kind/name` (one at a time for each name).
    pub fn begin(&self, kind: &str, name: &str) -> bool {
        self.busy
            .lock()
            .map(|mut b| b.insert(format!("{kind}/{name}")))
            .unwrap_or(false)
    }

    pub fn end(&self, kind: &str, name: &str) {
        if let Ok(mut b) = self.busy.lock() {
            b.remove(&format!("{kind}/{name}"));
        }
    }
}

// --- What a request may reach ------------------------------------------------------------

/// Whether `url` is a place an icon may be fetched from: http or https, a named host (not a
/// loopback, private or link-local address, not `localhost` or a local name) unless the policy
/// allows this machine.
pub fn allowed(url: &Url, policy: Policy) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    if policy.allow_local {
        return url.host().is_some();
    }
    if policy.allow_host.is_some() && url.host_str() == policy.allow_host {
        return true;
    }
    match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            !(d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || d.ends_with(".internal")
                || d.ends_with(".lan")
                || !d.contains('.'))
        }
        Some(url::Host::Ipv4(ip)) => !crate::bypass::is_local(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => !crate::bypass::is_local(IpAddr::V6(ip)),
        None => false,
    }
}

/// The client for fetching: `proxy` is the core's mixed port while it runs (so the site's own
/// route applies); without it the request goes out directly. Redirects are few and checked.
pub fn client(proxy: Option<&str>, policy: Policy) -> Result<reqwest::Client, String> {
    let redirects = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if !allowed(attempt.url(), policy) {
            attempt.error("redirected somewhere an icon is not taken from")
        } else {
            attempt.follow()
        }
    });
    let mut b = reqwest::Client::builder()
        .user_agent(concat!("Kairo/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(redirects);
    b = match proxy {
        Some(p) => b.proxy(reqwest::Proxy::all(p).map_err(|e| e.to_string())?),
        None => b.no_proxy(),
    };
    b.build().map_err(|e| e.to_string())
}

/// One GET, read to at most `limit` bytes: the final address and the body.
async fn get_limited(
    client: &reqwest::Client,
    url: &Url,
    limit: usize,
    policy: Policy,
) -> Result<(Url, Vec<u8>), String> {
    if !allowed(url, policy) {
        return Err("not a place an icon is taken from".into());
    }
    let resp = client
        .get(url.clone())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    if resp.content_length().is_some_and(|n| n as usize > limit) {
        return Err("too large".into());
    }
    let final_url = resp.url().clone();
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if body.len() + chunk.len() > limit {
            return Err("too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok((final_url, body))
}

// --- Finding the icon's address ----------------------------------------------------------

/// An icon a page declares.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub url: Url,
    /// The size it declares (the larger side), or a guess by its kind.
    pub size: u32,
}

/// The attributes of the `<link>` tags of `html` that name an icon, best first: the touch icon,
/// then the largest declared size, an SVG counting as large; `base` resolves relative addresses.
pub fn icon_links(html: &str, base: &Url) -> Vec<Candidate> {
    let lower = html.to_ascii_lowercase();
    let mut out: Vec<Candidate> = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<link") {
        let start = at + i + 5;
        let Some(len) = lower[start..].find('>') else {
            break;
        };
        let tag = &html[start..start + len];
        at = start + len;
        let attrs = attributes(tag);
        let get = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let rel = get("rel").unwrap_or("").to_ascii_lowercase();
        let rels: Vec<&str> = rel.split_whitespace().collect();
        let touch = rels.contains(&"apple-touch-icon") || rels.contains(&"apple-touch-icon-precomposed");
        if !(rels.contains(&"icon") || touch) {
            continue;
        }
        let Some(href) = get("href").map(str::trim).filter(|h| !h.is_empty()) else {
            continue;
        };
        let Ok(url) = base.join(href) else {
            continue;
        };
        let svg = get("type").is_some_and(|t| t.contains("svg")) || url.path().ends_with(".svg");
        let declared = get("sizes")
            .into_iter()
            .flat_map(|s| s.split_whitespace())
            .filter_map(|s| s.split(['x', 'X']).next()?.parse::<u32>().ok())
            .max();
        let size = if svg {
            256
        } else if let Some(d) = declared {
            d
        } else if touch {
            180
        } else {
            32
        };
        if !out.iter().any(|c| c.url == url) {
            out.push(Candidate { url, size });
        }
    }
    // Large ones first, but a 1000-pixel one is no better than a 256 one.
    out.sort_by_key(|c| std::cmp::Reverse(c.size.min(256)));
    out
}

/// `name="value"`, `name='value'` and `name=value` pairs of a tag, names in lower case.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let b = tag.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let s = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' {
            i += 1;
        }
        let name = tag[s..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                let s = i;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                value = tag[s..i].to_owned();
                i += 1;
            } else {
                let s = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = tag[s..i].to_owned();
            }
        }
        if !name.is_empty() {
            out.push((name, value));
        }
    }
    out
}

// --- Making a picture of what came back --------------------------------------------------

/// Looks at `bytes` and gives back a small PNG (any raster format, an `.ico` included, decoded
/// with limits so a huge picture is refused rather than unpacked) or an SVG that has nothing
/// active in it. Anything else is an error: the file is not a picture.
pub fn normalize(bytes: &[u8]) -> Result<Icon, String> {
    if bytes.is_empty() {
        return Err("empty".into());
    }
    if let Some(svg) = clean_svg(bytes) {
        return Ok(Icon {
            ext: "svg",
            bytes: svg.into_bytes(),
        });
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    reader.limits(limits);
    if reader.format().is_none() {
        return Err("not a picture".into());
    }
    let img = reader.decode().map_err(|e| format!("not a picture: {e}"))?;
    if img.width() < 8 || img.height() < 8 {
        return Err("too small to be an icon".into());
    }
    let img = if img.width().max(img.height()) > SIDE {
        img.resize(SIDE, SIDE, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(Icon { ext: "png", bytes: out })
}

/// An SVG from the system's own icon theme or app: taken as it is (it is the person's own file,
/// not something a site sent), within a size.
pub fn trusted(bytes: &[u8]) -> Option<Icon> {
    if bytes.len() > MAX_ICON {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    text.contains("<svg").then(|| Icon {
        ext: "svg",
        bytes: bytes.to_vec(),
    })
}

/// The SVG in `bytes` if it is one and has nothing that runs or reaches out: no script, no event
/// handler, no foreign object, no `<image>` or external reference, no entity.
fn clean_svg(bytes: &[u8]) -> Option<String> {
    if bytes.len() > 100 * 1024 {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let low = text.to_ascii_lowercase();
    if !low.contains("<svg") {
        return None;
    }
    let trimmed = low.trim_start_matches('\u{feff}').trim_start();
    if !(trimmed.starts_with("<svg") || trimmed.starts_with("<?xml") || trimmed.starts_with("<!--") || trimmed.starts_with("<!doctype svg")) {
        return None;
    }
    const FORBIDDEN: [&str; 14] = [
        "<script", "javascript:", "<foreignobject", "<image", "<use", "<!entity", "<iframe", "<embed",
        "<style", "http://", "https://", "xlink:href=\"http", " onload", " onclick",
    ];
    for f in FORBIDDEN {
        // The namespace is the one address an SVG has to carry.
        let rest = low.replace("http://www.w3.org/2000/svg", "").replace("http://www.w3.org/1999/xlink", "");
        if rest.contains(f) {
            return None;
        }
    }
    Some(text.to_owned())
}

// --- Fetching a site's icon -------------------------------------------------------------

/// The icon of a site: from its page's declaration, else `/favicon.ico`, over each of `origins`
/// (`https://example.com`, then `http://example.com`) until one gives a picture.
pub async fn fetch_site(
    client: &reqwest::Client,
    origins: &[String],
    policy: Policy,
) -> Result<Icon, String> {
    let mut last = "no icon".to_owned();
    for origin in origins {
        let Ok(base) = Url::parse(&format!("{}/", origin.trim_end_matches('/'))) else {
            continue;
        };
        let mut candidates: Vec<Candidate> = Vec::new();
        match get_limited(client, &base, MAX_PAGE, policy).await {
            Ok((final_url, page)) => {
                let html = String::from_utf8_lossy(&page);
                candidates = icon_links(&html, &final_url);
            }
            Err(e) => last = e,
        }
        if let Ok(fav) = base.join("/favicon.ico")
            && !candidates.iter().any(|c| c.url == fav)
        {
            candidates.push(Candidate { url: fav, size: 16 });
        }
        for c in candidates.into_iter().take(5) {
            match get_limited(client, &c.url, MAX_ICON, policy).await {
                Ok((_, bytes)) => match normalize(&bytes) {
                    Ok(icon) => return Ok(icon),
                    Err(e) => last = e,
                },
                Err(e) => last = e,
            }
        }
    }
    Err(last)
}

/// Where a site's icon is asked for: https first, then http.
pub fn site_origins(domain: &str) -> Vec<String> {
    vec![format!("https://{domain}"), format!("http://{domain}")]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 30, 30, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn names_become_file_names() {
        assert_eq!(file_key("GitHub.com").as_deref(), Some("github.com"));
        assert_eq!(file_key("../etc/passwd").as_deref(), Some("_etc_passwd"));
        assert_eq!(file_key("a/b").as_deref(), Some("a_b"));
        assert_eq!(file_key(" ").as_deref(), None);
        assert!(file_key("..").is_none());
    }

    #[test]
    fn page_links_come_best_first_and_resolve_against_the_page() {
        let base = Url::parse("https://example.com/app/").unwrap();
        let html = r#"<html><head>
            <link rel="stylesheet" href="/a.css">
            <link rel="icon" href="/favicon-16.png" sizes="16x16">
            <LINK REL='shortcut icon' HREF=icons/32.png sizes="32x32">
            <link rel="apple-touch-icon" href="/touch.png">
            <link rel="icon" type="image/svg+xml" href="logo.svg">
            <link rel="icon" href="/favicon-16.png" sizes="16x16">
            </head>"#;
        let links = icon_links(html, &base);
        let urls: Vec<&str> = links.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://example.com/app/logo.svg",
                "https://example.com/touch.png",
                "https://example.com/app/icons/32.png",
                "https://example.com/favicon-16.png",
            ]
        );
    }

    #[test]
    fn only_public_http_places_are_allowed() {
        let ok = |u: &str| allowed(&Url::parse(u).unwrap(), Policy::default());
        assert!(ok("https://example.com/x.png"));
        assert!(ok("http://cdn.example.com/x.png"));
        assert!(ok("https://8.8.8.8/x.png"));
        assert!(!ok("ftp://example.com/x.png"));
        assert!(!ok("file:///etc/passwd"));
        assert!(!ok("data:image/png;base64,AAAA"));
        assert!(!ok("http://localhost/x.png"));
        assert!(!ok("http://127.0.0.1/x.png"));
        assert!(!ok("http://192.168.1.1/x.png"));
        assert!(!ok("http://10.0.0.5/x.png"));
        assert!(!ok("http://169.254.169.254/latest/meta-data"));
        assert!(!ok("http://[::1]/x.png"));
        assert!(!ok("http://printer.local/x.png"));
        assert!(!ok("http://intranet/x.png"));
        assert!(allowed(
            &Url::parse("http://127.0.0.1:9/x.png").unwrap(),
            Policy { allow_local: true, allow_host: None }
        ));
    }

    #[test]
    fn a_picture_is_made_a_small_png_and_other_things_are_refused() {
        let big = normalize(&png(512, 256)).unwrap();
        assert_eq!(big.ext, "png");
        let back = image::load_from_memory(&big.bytes).unwrap();
        assert_eq!((back.width(), back.height()), (128, 64));
        assert_eq!(normalize(&png(32, 32)).unwrap().ext, "png");
        // Garbage, text, an empty file, a speck.
        assert!(normalize(b"").is_err());
        assert!(normalize(b"<html>not a picture</html>").is_err());
        assert!(normalize(&[0u8; 400]).is_err());
        assert!(normalize(&png(2, 2)).is_err());
        // A PNG cut short is not a picture.
        let p = png(64, 64);
        assert!(normalize(&p[..p.len() / 2]).is_err());
    }

    #[test]
    fn an_ico_is_read_and_a_picture_too_wide_is_refused_without_unpacking_it() {
        // An .ico holding one PNG.
        let p = png(48, 48);
        let mut ico = vec![0, 0, 1, 0, 1, 0, 48, 48, 0, 0, 1, 0, 32, 0];
        ico.extend_from_slice(&(p.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&p);
        let icon = normalize(&ico).unwrap();
        assert_eq!(icon.ext, "png");
        // A PNG that says it is 60000 pixels wide.
        let mut bomb = png(16, 16);
        bomb[16..20].copy_from_slice(&60000u32.to_be_bytes());
        assert!(normalize(&bomb).is_err());
    }

    #[test]
    fn an_svg_is_kept_only_when_nothing_in_it_runs_or_reaches_out() {
        let ok = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="#e25"/></svg>"##;
        assert_eq!(normalize(ok).unwrap().ext, "svg");
        for bad in [
            r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" onload="x()"></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="http://evil.example/x.png"/></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><foreignObject/></svg>"#,
            r#"<!DOCTYPE svg [<!ENTITY a "b">]><svg xmlns="http://www.w3.org/2000/svg"/>"#,
            r#"<html><svg xmlns="http://www.w3.org/2000/svg"/></html>"#,
        ] {
            assert!(normalize(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_folder_remembers_pictures_and_failures() {
        let dir = tempfile::tempdir().unwrap();
        let icons = Icons::new(dir.path().to_path_buf());
        let now = SystemTime::now();
        assert_eq!(icons.look("sites", "example.com", now), Cached::Unknown);
        icons.store_none("sites", "example.com");
        assert_eq!(icons.look("sites", "example.com", now), Cached::Missing);
        // A failure is tried again after three days.
        assert_eq!(
            icons.look("sites", "example.com", now + RETRY + Duration::from_secs(60)),
            Cached::Unknown
        );
        let icon = Icon { ext: "png", bytes: png(32, 32) };
        let path = icons.store("sites", "Example.com", &icon).unwrap();
        assert_eq!(
            icons.look("sites", "example.com", now),
            Cached::Have { path: path.clone(), stale: false }
        );
        // The failure marker is gone, and an old picture is still shown while it is looked at again.
        assert!(!dir.path().join("sites/example.com.none").exists());
        assert!(matches!(
            icons.look("sites", "example.com", now + KEEP + Duration::from_secs(60)),
            Cached::Have { stale: true, .. }
        ));
        // One fetch at a time for a name.
        assert!(icons.begin("sites", "example.com"));
        assert!(!icons.begin("sites", "example.com"));
        icons.end("sites", "example.com");
        assert!(icons.begin("sites", "example.com"));
    }
}
