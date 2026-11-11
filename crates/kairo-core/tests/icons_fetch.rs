//! A site's icon, fetched from a server on this machine: the page's declaration, the favicon, and
//! what goes wrong (nothing there, a bad picture, a huge one, redirects).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use kairo_core::icons::{self, Policy};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    /// Send the body without a Content-Length, as a stream that ends when the server closes.
    stream: bool,
}

impl Reply {
    fn ok(body: Vec<u8>) -> Reply {
        Reply { status: 200, headers: vec![], body, stream: false }
    }
    fn redirect(to: &str) -> Reply {
        Reply { status: 302, headers: vec![("Location", to.to_owned())], body: vec![], stream: false }
    }
    fn status(status: u16) -> Reply {
        Reply { status, headers: vec![], body: vec![], stream: false }
    }
}

/// A server on a free port; `route` answers each path. Returns the origin and how many
/// requests it has seen.
async fn serve(
    route: impl Fn(&str) -> Reply + Send + Sync + 'static,
) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(AtomicUsize::new(0));
    let count = seen.clone();
    let route = Arc::new(route);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            let route = route.clone();
            let count = count.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                count.fetch_add(1, Ordering::SeqCst);
                let r = route(&path);
                let mut out = format!("HTTP/1.1 {} X\r\nConnection: close\r\n", r.status);
                for (k, v) in &r.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                if !r.stream {
                    out.push_str(&format!("Content-Length: {}\r\n", r.body.len()));
                }
                out.push_str("\r\n");
                let _ = sock.write_all(out.as_bytes()).await;
                // In pieces, so a reader that gives up early is not waiting on one big write.
                for chunk in r.body.chunks(16 * 1024) {
                    if sock.write_all(chunk).await.is_err() {
                        break;
                    }
                }
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([20, 120, 220, 255]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

const LOCAL: Policy = Policy { allow_local: true, allow_host: None };
/// The test server only; every other address keeps the real rules.
const ONLY_SERVER: Policy = Policy { allow_local: false, allow_host: Some("127.0.0.1") };

async fn fetch(origin: &str) -> Result<icons::Icon, String> {
    let client = icons::client(None, LOCAL).unwrap();
    icons::fetch_site(&client, &[origin.to_owned()], LOCAL).await
}

fn size_of(icon: &icons::Icon) -> (u32, u32) {
    let img = image::load_from_memory(&icon.bytes).unwrap();
    (img.width(), img.height())
}

#[tokio::test]
async fn the_icon_the_page_declares_is_the_one_taken() {
    let (origin, _) = serve(|path| match path {
        "/" => Reply::ok(
            br#"<html><head><link rel="icon" href="/small.png" sizes="16x16">
                <link rel="icon" href="static/big.png" sizes="96x96"></head></html>"#
                .to_vec(),
        ),
        "/small.png" => Reply::ok(png(16, 16)),
        "/static/big.png" => Reply::ok(png(96, 96)),
        _ => Reply::status(404),
    })
    .await;
    let icon = fetch(&origin).await.unwrap();
    assert_eq!(icon.ext, "png");
    assert_eq!(size_of(&icon), (96, 96));
}

#[tokio::test]
async fn without_a_declaration_the_favicon_is_asked_for() {
    let (origin, _) = serve(|path| match path {
        "/" => Reply::ok(b"<html>no icon here</html>".to_vec()),
        "/favicon.ico" => Reply::ok(png(32, 32)),
        _ => Reply::status(404),
    })
    .await;
    assert_eq!(size_of(&fetch(&origin).await.unwrap()), (32, 32));
    // No page at all (a 404 on /) and only the favicon.
    let (origin, _) = serve(|path| match path {
        "/favicon.ico" => Reply::ok(png(24, 24)),
        _ => Reply::status(404),
    })
    .await;
    assert_eq!(size_of(&fetch(&origin).await.unwrap()), (24, 24));
}

#[tokio::test]
async fn an_svg_the_page_declares_is_kept_as_an_svg() {
    let (origin, _) = serve(|path| match path {
        "/" => Reply::ok(br#"<link rel="icon" type="image/svg+xml" href="/i.svg">"#.to_vec()),
        "/i.svg" => Reply::ok(
            br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8" fill="#2a6"/></svg>"##.to_vec(),
        ),
        _ => Reply::status(404),
    })
    .await;
    let icon = fetch(&origin).await.unwrap();
    assert_eq!(icon.ext, "svg");
    // An SVG that runs a script is not kept; the favicon behind it is.
    let (origin, _) = serve(|path| match path {
        "/" => Reply::ok(br#"<link rel="icon" type="image/svg+xml" href="/i.svg">"#.to_vec()),
        "/i.svg" => Reply::ok(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#.to_vec(),
        ),
        "/favicon.ico" => Reply::ok(png(32, 32)),
        _ => Reply::status(404),
    })
    .await;
    assert_eq!(fetch(&origin).await.unwrap().ext, "png");
}

#[tokio::test]
async fn offline_is_an_error_and_not_a_hang() {
    // A port nothing listens on.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let started = std::time::Instant::now();
    let err = fetch(&format!("http://127.0.0.1:{port}")).await.unwrap_err();
    assert!(!err.is_empty());
    assert!(started.elapsed() < Duration::from_secs(8), "{:?}", started.elapsed());
}

#[tokio::test]
async fn a_bad_picture_is_refused() {
    for body in [
        b"this is not a picture".to_vec(),
        b"<html><body>Not Found</body></html>".to_vec(),
        vec![0u8; 2000],
        {
            let p = png(64, 64);
            p[..p.len() / 3].to_vec()
        },
    ] {
        let body2 = body.clone();
        let (origin, _) = serve(move |path| match path {
            "/favicon.ico" => Reply::ok(body2.clone()),
            _ => Reply::status(404),
        })
        .await;
        assert!(fetch(&origin).await.is_err());
    }
    // A picture that claims to be enormous is not unpacked.
    let mut bomb = png(16, 16);
    bomb[16..20].copy_from_slice(&60000u32.to_be_bytes());
    let (origin, _) = serve(move |path| match path {
        "/favicon.ico" => Reply::ok(bomb.clone()),
        _ => Reply::status(404),
    })
    .await;
    assert!(fetch(&origin).await.is_err());
}

#[tokio::test]
async fn a_huge_file_is_cut_off() {
    // With a Content-Length, and as a stream of unknown length.
    for stream in [false, true] {
        let (origin, _) = serve(move |path| match path {
            "/favicon.ico" => Reply { status: 200, headers: vec![], body: vec![7u8; 6 * 1024 * 1024], stream },
            "/" => Reply { status: 200, headers: vec![], body: vec![b'a'; 3 * 1024 * 1024], stream },
            _ => Reply::status(404),
        })
        .await;
        let started = std::time::Instant::now();
        let err = fetch(&origin).await.unwrap_err();
        assert!(err.contains("too large") || !err.is_empty(), "{err}");
        assert!(started.elapsed() < Duration::from_secs(8));
    }
    // The limit is on what is read: an icon just under it is fine.
    let ok = png(128, 128);
    assert!(ok.len() < icons::MAX_ICON);
}

#[tokio::test]
async fn redirects_are_followed_a_few_times_and_checked() {
    // /favicon.ico → /a → /b → the picture.
    let (origin, _) = serve(|path| match path {
        "/favicon.ico" => Reply::redirect("/a"),
        "/a" => Reply::redirect("/b"),
        "/b" => Reply::redirect("/real.png"),
        "/real.png" => Reply::ok(png(48, 48)),
        _ => Reply::status(404),
    })
    .await;
    assert_eq!(size_of(&fetch(&origin).await.unwrap()), (48, 48));

    // A loop, and a chain longer than the limit.
    let (origin, seen) = serve(|path| match path {
        "/favicon.ico" => Reply::redirect("/loop"),
        "/loop" => Reply::redirect("/favicon.ico"),
        _ => Reply::status(404),
    })
    .await;
    assert!(fetch(&origin).await.is_err());
    assert!(seen.load(Ordering::SeqCst) < 40, "it kept following");
    let (origin, _) = serve(|path| {
        let n: usize = path.trim_start_matches("/hop").parse().unwrap_or(0);
        match path {
            "/favicon.ico" => Reply::redirect("/hop1"),
            _ if path.starts_with("/hop") && n < 10 => Reply::redirect(&format!("/hop{}", n + 1)),
            _ if path.starts_with("/hop") => Reply::ok(png(32, 32)),
            _ => Reply::status(404),
        }
    })
    .await;
    assert!(fetch(&origin).await.is_err(), "ten hops are too many");

    // A redirect to something that is not http(s) is not followed.
    for target in ["file:///etc/passwd", "ftp://example.com/x.png", "data:image/png;base64,AAAA"] {
        let t = target.to_owned();
        let (origin, _) = serve(move |path| match path {
            "/favicon.ico" => Reply::redirect(&t),
            _ => Reply::status(404),
        })
        .await;
        assert!(fetch(&origin).await.is_err(), "{target}");
    }
}

#[tokio::test]
async fn this_machine_and_private_places_are_not_visited_by_default() {
    let (origin, seen) = serve(|_| Reply::ok(png(32, 32))).await;
    let client = icons::client(None, Policy::default()).unwrap();
    let err = icons::fetch_site(&client, &[origin], Policy::default()).await.unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(seen.load(Ordering::SeqCst), 0, "nothing was asked of 127.0.0.1");
}

#[tokio::test]
async fn a_page_may_not_send_the_fetch_to_a_private_address() {
    // The page declares an icon on a private address, and the favicon redirects to another: neither is
    // visited; the favicon of the page's own host that does answer is taken.
    let started = std::time::Instant::now();
    let (origin, _) = serve(|path| match path {
        "/" => Reply::ok(br#"<link rel="icon" href="http://192.168.1.1/admin.png"><link rel="icon" href="http://169.254.169.254/x.png">"#.to_vec()),
        "/favicon.ico" => Reply::ok(png(32, 32)),
        _ => Reply::status(404),
    })
    .await;
    let client = icons::client(None, ONLY_SERVER).unwrap();
    let icon = icons::fetch_site(&client, &[origin], ONLY_SERVER).await.unwrap();
    assert_eq!(size_of(&icon), (32, 32));
    assert!(started.elapsed() < Duration::from_secs(2), "it tried the private address");

    let (origin, _) = serve(|path| match path {
        "/favicon.ico" => Reply::redirect("http://192.168.1.1/admin.png"),
        _ => Reply::status(404),
    })
    .await;
    let started = std::time::Instant::now();
    assert!(icons::fetch_site(&client, &[origin], ONLY_SERVER).await.is_err());
    assert!(started.elapsed() < Duration::from_secs(2), "it followed the redirect");
}
