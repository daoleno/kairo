//! Kairo updates itself, the bundled mihomo core included (one updater).
//!
//! Every release carries `kairo-update.json` (the version and, per package,
//! its file name and SHA-256) and `kairo-update.json.sig`, an Ed25519
//! signature of the manifest's exact bytes made in the release workflow. The
//! public key is compiled in below. Kairo:
//!
//! 1. fetches the latest manifest (on launch, then every few hours) and
//!    checks its signature;
//! 2. if it names a newer version, downloads this installation's package and
//!    checks its SHA-256 against the signed manifest and the release's
//!    `SHA256SUMS`;
//! 3. keeps it under `<home>/update/` and offers "Restart to Update". That
//!    tap installs it and starts the new Kairo.
//!
//! How it installs depends on how Kairo was installed ([`Install`]): the macOS
//! app bundle is swapped for the new one; a tarball install (`install.sh`,
//! `~/.local`) has its files replaced in place; the .deb goes through
//! `pkexec apt-get install` (one password prompt). Anything else gets a link to
//! the release.
//!
//! `KAIRO_UPDATE_URL` points the updater at another place that serves the same
//! files (tests). The key cannot be changed. `KAIRO_NO_UPDATE=1` turns the
//! background checks off (tests, isolated runs).

use std::path::{Path, PathBuf};
use std::time::Duration;

use ed25519_dalek::{Signature, VerifyingKey};
use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::core_bin::is_newer;
use crate::paths::write_atomic;
use crate::service::Service;

/// The release workflow's signing key (public half; the private half is the
/// `KAIRO_UPDATE_KEY` Actions secret).
const PUBLIC_KEY: [u8; 32] = hex_literal(UPDATE_PUBLIC_KEY_HEX);
const UPDATE_PUBLIC_KEY_HEX: &str = "336a3afab474faf88ed05e20cfe862fde8e0ad0f65abbc0e9fc88a222add86bf";

const LATEST: &str = "https://github.com/daoleno/kairo/releases/latest/download";
const RELEASES: &str = "https://github.com/daoleno/kairo/releases";
pub const MANIFEST: &str = "kairo-update.json";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const fn hex_literal(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    let mut out = [0u8; 32];
    if b.len() != 64 {
        return out;
    }
    let mut i = 0;
    while i < 32 {
        out[i] = (nibble(b[2 * i]) << 4) | nibble(b[2 * i + 1]);
        i += 1;
    }
    out
}

const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

/// What the updater is doing, for the UI.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    /// When the last check found nothing newer (`HH:MM`, local time).
    UpToDate { checked: String },
    Downloading { version: String, progress: Option<f32> },
    /// Downloaded and verified: one tap installs it.
    Ready { version: String },
    /// This installation cannot update itself: the release page.
    Manual { version: String, url: String },
    Installing { version: String },
    Failed { message: String },
}

/// The signed manifest.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    pub version: String,
    /// Package kind (see [`Install::package_key`]) → file.
    pub packages: std::collections::BTreeMap<String, Package>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Package {
    pub name: String,
    pub sha256: String,
}

/// How this copy of Kairo was installed, which decides how it is replaced.
#[derive(Debug, Clone, PartialEq)]
pub enum Install {
    /// `…/Kairo.app` (macOS).
    MacApp { bundle: PathBuf },
    /// `<prefix>/bin/kairo` and `<prefix>/lib/kairo/mihomo`, writable by the user.
    Prefix { prefix: PathBuf },
    /// `/usr/bin/kairo` from the .deb.
    Deb,
    /// Anything else (a development build, a system-wide copy made with sudo).
    Other,
}

impl Install {
    pub fn detect() -> Install {
        let Ok(exe) = std::env::current_exe() else { return Install::Other };
        let exe = exe.canonicalize().unwrap_or(exe);
        Self::detect_for(&exe, Path::new("/var/lib/dpkg/info/kairo.list"))
    }

    pub fn detect_for(exe: &Path, dpkg_list: &Path) -> Install {
        if cfg!(target_os = "macos") {
            let bundle = exe.parent().and_then(Path::parent).and_then(Path::parent);
            if let Some(bundle) = bundle
                && bundle.extension().is_some_and(|e| e == "app")
                && exe.parent().is_some_and(|d| d.ends_with("Contents/MacOS"))
            {
                return Install::MacApp { bundle: bundle.to_owned() };
            }
            return Install::Other;
        }
        if exe == Path::new("/usr/bin/kairo") {
            let owned = std::fs::read_to_string(dpkg_list).is_ok_and(|l| l.lines().any(|f| f == "/usr/bin/kairo"));
            return if owned { Install::Deb } else { Install::Other };
        }
        let Some(bin) = exe.parent() else { return Install::Other };
        let Some(prefix) = bin.parent() else { return Install::Other };
        if bin.file_name().is_some_and(|n| n == "bin")
            && prefix.join("lib/kairo/mihomo").is_file()
            && writable(bin)
            && writable(&prefix.join("lib/kairo"))
        {
            return Install::Prefix { prefix: prefix.to_owned() };
        }
        Install::Other
    }

    /// The manifest's key for this installation's package.
    pub fn package_key(&self) -> Option<String> {
        let arch = std::env::consts::ARCH;
        match self {
            Install::MacApp { .. } => Some("macos-universal-zip".into()),
            Install::Prefix { .. } => Some(format!("linux-{arch}-tar")),
            Install::Deb => Some(format!("linux-{arch}-deb")),
            Install::Other => None,
        }
    }
}

fn writable(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()) else { return false };
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        false
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum VerifyError {
    #[error("The update's signature is not valid")]
    Signature,
    #[error("The update manifest is unreadable: {0}")]
    Manifest(String),
    #[error("The download's checksum does not match ({0})")]
    Checksum(String),
}

/// Checks the manifest's signature (hex in `.sig`) and reads it.
pub fn verify_manifest(bytes: &[u8], sig_hex: &str) -> Result<Manifest, VerifyError> {
    verify_with(&PUBLIC_KEY, bytes, sig_hex)
}

fn verify_with(key: &[u8; 32], bytes: &[u8], sig_hex: &str) -> Result<Manifest, VerifyError> {
    let key = VerifyingKey::from_bytes(key).map_err(|_| VerifyError::Signature)?;
    let sig: [u8; 64] = hex::decode(sig_hex.trim())
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or(VerifyError::Signature)?;
    // Strict: also refuses weak keys and malleable signatures.
    key.verify_strict(bytes, &Signature::from_bytes(&sig)).map_err(|_| VerifyError::Signature)?;
    serde_json::from_slice(bytes).map_err(|e| VerifyError::Manifest(e.to_string()))
}

/// Checks `bytes` against the signed SHA-256 and the release's `SHA256SUMS`.
pub fn verify_package(bytes: &[u8], package: &Package, sums: &str) -> Result<(), VerifyError> {
    let actual = hex::encode(Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(&package.sha256) {
        return Err(VerifyError::Checksum(format!("expected {}, got {actual}", package.sha256)));
    }
    let listed = sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (sum, name) = (it.next()?, it.next()?);
        (name.trim_start_matches('*') == package.name).then(|| sum.to_owned())
    });
    match listed {
        Some(s) if s.eq_ignore_ascii_case(&actual) => Ok(()),
        Some(s) => Err(VerifyError::Checksum(format!("SHA256SUMS says {s}, got {actual}"))),
        None => Err(VerifyError::Checksum(format!("{} is not in SHA256SUMS", package.name))),
    }
}

fn base_url() -> String {
    std::env::var("KAIRO_UPDATE_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| LATEST.into())
}

/// Where a release's files are.
fn file_url(version: &str, name: &str) -> String {
    match std::env::var("KAIRO_UPDATE_URL").ok().filter(|u| !u.is_empty()) {
        Some(base) => format!("{}/{name}", base.trim_end_matches('/')),
        None => format!("{RELEASES}/download/v{version}/{name}"),
    }
}

fn client(proxy: Option<&str>) -> Result<reqwest::Client, String> {
    let mut b = reqwest::Client::builder()
        .user_agent(concat!("Kairo/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15));
    if let Some(p) = proxy {
        b = b.proxy(reqwest::Proxy::all(p).map_err(|e| e.to_string())?);
    }
    b.build().map_err(|e| e.to_string())
}

async fn get(url: &str, proxy: Option<&str>, mut progress: impl FnMut(u64, u64)) -> Result<Vec<u8>, String> {
    let resp = client(proxy)?
        .get(url)
        .timeout(Duration::from_secs(900))
        .send()
        .await
        .map_err(|e| format!("Could not reach {}: {e}", host(url)))?;
    if !resp.status().is_success() {
        return Err(format!("{} answered HTTP {}", host(url), resp.status().as_u16()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut out = Vec::with_capacity(total as usize);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
        progress(out.len() as u64, total);
    }
    Ok(out)
}

fn host(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| url.to_owned())
}

/// What is waiting in `<home>/update/`.
#[derive(Debug, Clone, serde::Serialize, Deserialize, PartialEq)]
struct Staged {
    version: String,
    file: String,
    sha256: String,
}

fn staged_info(dir: &Path) -> Option<Staged> {
    std::fs::read(dir.join("staged.json")).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// Starts the background checks (unless `KAIRO_NO_UPDATE` is set). A
/// download left from an earlier run is offered again; one already
/// installed is cleaned up.
pub fn spawn_checks(svc: Service) {
    let dir = svc.paths().update_dir();
    match staged_info(&dir) {
        Some(s) if is_newer(&s.version, VERSION) && dir.join(&s.file).is_file() => {
            svc.set_update(UpdateStatus::Ready { version: s.version });
        }
        Some(_) => {
            let _ = std::fs::remove_dir_all(&dir);
        }
        None => {}
    }
    if std::env::var_os("KAIRO_NO_UPDATE").is_some_and(|v| !v.is_empty()) {
        return;
    }
    tokio::spawn(async move {
        // Let the core come up first: a download may go through it.
        tokio::time::sleep(Duration::from_secs(8)).await;
        loop {
            if let Err(e) = check(&svc).await {
                log::warn!("update check: {e}");
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// Looks for a newer Kairo and downloads it. `Ok(Some(version))` when one is
/// ready (or can only be downloaded by hand), `Ok(None)` when up to date.
pub async fn check(svc: &Service) -> Result<Option<String>, String> {
    let _busy = match svc.update_lock().try_lock() {
        Ok(g) => g,
        Err(_) => return Ok(None),
    };
    let previous = svc.snapshot().update;
    if matches!(previous, UpdateStatus::Installing { .. }) {
        return Ok(None);
    }
    // With an update waiting, a check only looks for something newer, quietly.
    let quiet = matches!(previous, UpdateStatus::Ready { .. });
    if !quiet {
        svc.set_update(UpdateStatus::Checking);
    }
    let result = check_inner(svc).await;
    match &result {
        Ok(None) if quiet => svc.set_update(previous),
        Ok(None) => svc.set_update(UpdateStatus::UpToDate { checked: crate::service::clock()[..5].to_owned() }),
        Ok(Some(_)) => {}
        Err(e) if quiet => {
            log::warn!("update: {e}");
            svc.set_update(previous);
        }
        Err(e) => svc.set_update(UpdateStatus::Failed { message: e.clone() }),
    }
    result
}

async fn check_inner(svc: &Service) -> Result<Option<String>, String> {
    let routes = svc.download_routes();
    let mut last_err = String::new();
    for proxy in &routes {
        match fetch_and_stage(svc, proxy.as_deref()).await {
            Ok(r) => return Ok(r),
            Err(Fetch::Network(e)) => last_err = e,
            Err(Fetch::Fatal(e)) => return Err(e),
        }
    }
    Err(last_err)
}

enum Fetch {
    /// Worth trying another route.
    Network(String),
    Fatal(String),
}

async fn fetch_and_stage(svc: &Service, proxy: Option<&str>) -> Result<Option<String>, Fetch> {
    let base = base_url();
    let base = base.trim_end_matches('/');
    let bytes = get(&format!("{base}/{MANIFEST}"), proxy, |_, _| {}).await.map_err(Fetch::Network)?;
    let sig = get(&format!("{base}/{MANIFEST}.sig"), proxy, |_, _| {}).await.map_err(Fetch::Network)?;
    let manifest = verify_manifest(&bytes, &String::from_utf8_lossy(&sig)).map_err(|e| Fetch::Fatal(e.to_string()))?;
    if !is_newer(&manifest.version, VERSION) {
        return Ok(None);
    }
    let version = manifest.version.clone();
    let dir = svc.paths().update_dir();
    if staged_info(&dir).is_some_and(|s| s.version == version && dir.join(&s.file).is_file()) {
        svc.set_update(UpdateStatus::Ready { version: version.clone() });
        return Ok(Some(version));
    }
    let install = Install::detect();
    let package = install.package_key().and_then(|k| manifest.packages.get(&k).cloned());
    let Some(package) = package else {
        svc.set_update(UpdateStatus::Manual { version: version.clone(), url: format!("{RELEASES}/tag/v{version}") });
        return Ok(Some(version));
    };
    svc.set_update(UpdateStatus::Downloading { version: version.clone(), progress: None });
    let sums = get(&file_url(&version, "SHA256SUMS"), proxy, |_, _| {}).await.map_err(Fetch::Network)?;
    let progress_svc = svc.clone();
    let progress_version = version.clone();
    let mut last = 0.0f32;
    let file = get(&file_url(&version, &package.name), proxy, move |done, total| {
        let p = if total > 0 { done as f32 / total as f32 } else { 0.0 };
        if p - last >= 0.02 || p >= 1.0 {
            last = p;
            progress_svc.set_update(UpdateStatus::Downloading { version: progress_version.clone(), progress: Some(p) });
        }
    })
    .await
    .map_err(Fetch::Network)?;
    verify_package(&file, &package, &String::from_utf8_lossy(&sums)).map_err(|e| Fetch::Fatal(e.to_string()))?;
    let _ = std::fs::remove_dir_all(&dir);
    let io = |e: std::io::Error| Fetch::Fatal(format!("Could not keep the update: {e}"));
    std::fs::create_dir_all(&dir).map_err(io)?;
    write_atomic(&dir.join(&package.name), &file).map_err(io)?;
    let staged = Staged { version: version.clone(), file: package.name.clone(), sha256: package.sha256.clone() };
    write_atomic(&dir.join("staged.json"), &serde_json::to_vec_pretty(&staged).expect("json")).map_err(io)?;
    log::info!("Kairo {version} downloaded and verified");
    svc.set_update(UpdateStatus::Ready { version: version.clone() });
    Ok(Some(version))
}

/// Installs the waiting update. Returns the executable to start: the caller
/// starts it (see [`relaunch`]) and quits.
pub async fn install(svc: &Service) -> Result<PathBuf, String> {
    let dir = svc.paths().update_dir();
    let staged = staged_info(&dir).ok_or("No update is waiting")?;
    let file = dir.join(&staged.file);
    let bytes = tokio::fs::read(&file).await.map_err(|e| e.to_string())?;
    // Checked again: it sat on disk since the download.
    let actual = hex::encode(Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(&staged.sha256) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("The downloaded update changed on disk; it was removed".into());
    }
    let install = Install::detect();
    let work = dir.join("work");
    let _ = std::fs::remove_dir_all(&work);
    let result = tokio::task::spawn_blocking(move || match install {
        Install::MacApp { bundle } => install_mac(&bundle, &file, &work),
        Install::Prefix { prefix } => install_prefix(&prefix, &bytes, &work),
        Install::Deb => install_deb(&file),
        Install::Other => Err("This copy of Kairo cannot update itself".into()),
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(dir.join("work"));
    result
}

#[cfg(target_os = "macos")]
fn run(cmd: &mut std::process::Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} failed: {}",
            cmd.get_program().to_string_lossy(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Unzips the new Kairo.app next to the old one, checks it, and swaps the two
/// with renames (the running process keeps its open files).
#[allow(unused_variables)]
fn install_mac(bundle: &Path, zip: &Path, work: &Path) -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let s = bundle.to_string_lossy();
        if s.contains("/AppTranslocation/") {
            return Err("Kairo is running from a temporary location. Move it to Applications, open it from there, and update again.".into());
        }
        let parent = bundle.parent().ok_or("no parent")?;
        if !writable(parent) {
            return Err(format!("Kairo cannot write to {}", parent.display()));
        }
        let pid = std::process::id();
        let stage = parent.join(format!(".Kairo-update-{pid}"));
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
        let cleanup = |e: String| {
            let _ = std::fs::remove_dir_all(&stage);
            e
        };
        run(Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(zip).arg(&stage)).map_err(cleanup)?;
        let new = stage.join("Kairo.app");
        run(Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(&new)).map_err(cleanup)?;
        if !new.join("Contents/MacOS/Kairo").is_file() || !new.join("Contents/MacOS/mihomo").is_file() {
            return Err(cleanup("The update is missing files".into()));
        }
        let old = parent.join(format!(".Kairo-old-{pid}.app"));
        std::fs::rename(bundle, &old).map_err(|e| cleanup(format!("Could not move the old Kairo aside: {e}")))?;
        if let Err(e) = std::fs::rename(&new, bundle) {
            let _ = std::fs::rename(&old, bundle);
            return Err(cleanup(format!("Could not put the new Kairo in place: {e}")));
        }
        let _ = std::fs::remove_dir_all(&old);
        let _ = std::fs::remove_dir_all(&stage);
        let _ = work;
        return Ok(bundle.join("Contents/MacOS/Kairo"));
    }
    #[allow(unreachable_code)]
    Err("not macOS".into())
}

/// Unpacks the tarball and replaces `bin/kairo`, `lib/kairo/*` and the docs
/// with renames. The launcher entry and icons stay (install.sh wrote the
/// prefix into the entry).
fn install_prefix(prefix: &Path, tar_gz: &[u8], work: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tar_gz));
    archive.set_preserve_permissions(true);
    archive.unpack(work).map_err(|e| format!("Could not unpack the update: {e}"))?;
    let top = std::fs::read_dir(work)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.join("bin/kairo").is_file())
        .ok_or("The update has no bin/kairo")?;
    if !top.join("lib/kairo/mihomo").is_file() {
        return Err("The update has no core".into());
    }
    let mut files = vec![PathBuf::from("lib/kairo/mihomo")];
    for sub in ["lib/kairo", "share/doc/kairo"] {
        if let Ok(rd) = std::fs::read_dir(top.join(sub)) {
            for e in rd.filter_map(Result::ok) {
                let rel = Path::new(sub).join(e.file_name());
                if e.path().is_file() && !files.contains(&rel) {
                    files.push(rel);
                }
            }
        }
    }
    // The launcher last: if anything before fails, the old one still runs.
    files.push(PathBuf::from("bin/kairo"));
    for rel in &files {
        replace_file(&top.join(rel), &prefix.join(rel))?;
    }
    Ok(prefix.join("bin/kairo"))
}

/// Copies `from` next to `to` and renames it over `to`.
fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    let dir = to.parent().ok_or("no parent")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".{}.update", to.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
    std::fs::copy(from, &tmp).map_err(|e| format!("{}: {e}", to.display()))?;
    std::fs::rename(&tmp, to).map_err(|e| format!("{}: {e}", to.display()))
}

/// `pkexec apt-get install <deb>`: one password prompt, dependencies resolved.
#[allow(unused_variables)]
fn install_deb(deb: &Path) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let apt = ["/usr/bin/apt-get", "/usr/bin/apt"].into_iter().find(|p| Path::new(p).exists());
        let mut cmd = Command::new("pkexec");
        match apt {
            Some(apt) => cmd.args([apt, "install", "-y", "--no-install-recommends"]).arg(deb),
            None => cmd.args(["/usr/bin/dpkg", "-i"]).arg(deb),
        };
        cmd.env("DEBIAN_FRONTEND", "noninteractive");
        let out = cmd.output().map_err(|e| format!("pkexec: {e}"))?;
        return match out.status.code() {
            Some(0) => Ok(PathBuf::from("/usr/bin/kairo")),
            Some(126) => Err("Cancelled".into()),
            Some(127) => Err("No polkit agent could ask for the password".into()),
            _ => Err(String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("apt-get failed").to_owned()),
        };
    }
    #[allow(unreachable_code)]
    Err("not Linux".into())
}

/// The flag a relaunched Kairo gets: wait for the old process to be gone.
pub const AFTER_FLAG: &str = "--after";

/// Starts `exe` in its own session, telling it to wait for this process to
/// end. It inherits the environment (the data directory, the session bus).
pub fn relaunch(exe: &Path) -> Result<(), String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(AFTER_FLAG).arg(std::process::id().to_string());
    cmd.stdin(std::process::Stdio::null());
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("Could not start the new Kairo: {e}"))
}

/// In a relaunched Kairo: waits (up to 30 s) until the process it replaces is gone.
pub fn wait_for_previous() {
    let args: Vec<String> = std::env::args().collect();
    let Some(pid) = args.iter().position(|a| a == AFTER_FLAG).and_then(|i| args.get(i + 1)).and_then(|p| p.parse::<i32>().ok()) else {
        return;
    };
    let start = std::time::Instant::now();
    while alive(pid) && start.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        if unsafe { libc::kill(pid, 0) } != 0 {
            return false;
        }
        // A zombie (exited, not yet reaped) is gone for our purposes.
        #[cfg(target_os = "linux")]
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            && let Some(state) = stat.rsplit(')').next().and_then(|r| r.split_whitespace().next())
        {
            return state != "Z";
        }
        true
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signed(key: &SigningKey, json: &str) -> String {
        hex::encode(key.sign(json.as_bytes()).to_bytes())
    }

    const JSON: &str = r#"{"version":"0.1.3","packages":{"linux-x86_64-tar":{"name":"Kairo-0.1.3-linux-x86_64.tar.gz","sha256":"ab"}},"future":1}"#;

    #[test]
    fn the_compiled_key_is_real() {
        assert_ne!(PUBLIC_KEY, [0u8; 32]);
        assert!(VerifyingKey::from_bytes(&PUBLIC_KEY).is_ok());
    }

    #[test]
    fn manifests_need_the_right_signature() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let public = key.verifying_key().to_bytes();
        let sig = signed(&key, JSON);
        let m = verify_with(&public, JSON.as_bytes(), &sig).unwrap();
        assert_eq!(m.version, "0.1.3");
        assert_eq!(m.packages["linux-x86_64-tar"].name, "Kairo-0.1.3-linux-x86_64.tar.gz");
        // A changed byte, another key, garbage: refused.
        let tampered = JSON.replace("0.1.3\",", "0.1.4\",");
        assert_eq!(verify_with(&public, tampered.as_bytes(), &sig), Err(VerifyError::Signature));
        let other = SigningKey::from_bytes(&[8u8; 32]);
        assert_eq!(verify_with(&public, JSON.as_bytes(), &signed(&other, JSON)), Err(VerifyError::Signature));
        assert_eq!(verify_with(&public, JSON.as_bytes(), "zz"), Err(VerifyError::Signature));
        // The release key does not accept a test signature.
        assert_eq!(verify_manifest(JSON.as_bytes(), &sig), Err(VerifyError::Signature));
    }

    /// A signature made by `openssl pkeyutl -sign -rawin` (as the release
    /// workflow does) verifies here.
    #[test]
    fn openssl_signatures_verify() {
        let Ok(dir) = tempfile::tempdir() else { return };
        let key = dir.path().join("k.pem");
        let ok = std::process::Command::new("openssl")
            .args(["genpkey", "-algorithm", "ed25519", "-out"])
            .arg(&key)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("skipped: no openssl");
            return;
        }
        let msg = dir.path().join("m.json");
        std::fs::write(&msg, JSON).unwrap();
        let sig = dir.path().join("m.sig");
        let signed = std::process::Command::new("openssl")
            .args(["pkeyutl", "-sign", "-rawin", "-inkey"])
            .arg(&key)
            .arg("-in")
            .arg(&msg)
            .arg("-out")
            .arg(&sig)
            .status()
            .is_ok_and(|s| s.success());
        if !signed {
            eprintln!("skipped: this openssl cannot sign raw Ed25519 (LibreSSL)");
            return;
        }
        let der = std::process::Command::new("openssl")
            .args(["pkey", "-pubout", "-outform", "DER", "-in"])
            .arg(&key)
            .output()
            .unwrap()
            .stdout;
        let public: [u8; 32] = der[der.len() - 32..].try_into().unwrap();
        let sig_hex = hex::encode(std::fs::read(&sig).unwrap());
        assert_eq!(verify_with(&public, JSON.as_bytes(), &sig_hex).unwrap().version, "0.1.3");
    }

    #[test]
    fn packages_are_checked_against_both_sums() {
        let bytes = b"package";
        let sum = hex::encode(Sha256::digest(bytes));
        let p = Package { name: "k.tar.gz".into(), sha256: sum.clone() };
        assert!(verify_package(bytes, &p, &format!("{sum}  k.tar.gz\nff  other\n")).is_ok());
        assert!(verify_package(b"other", &p, &format!("{sum}  k.tar.gz\n")).is_err());
        assert!(verify_package(bytes, &p, "ff  k.tar.gz\n").is_err());
        assert!(verify_package(bytes, &p, "").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detects_how_it_was_installed() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("p");
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::create_dir_all(prefix.join("lib/kairo")).unwrap();
        std::fs::write(prefix.join("bin/kairo"), "").unwrap();
        let none = dir.path().join("no-dpkg");
        // No core next to it: not an install.sh layout.
        assert_eq!(Install::detect_for(&prefix.join("bin/kairo"), &none), Install::Other);
        std::fs::write(prefix.join("lib/kairo/mihomo"), "").unwrap();
        assert_eq!(Install::detect_for(&prefix.join("bin/kairo"), &none), Install::Prefix { prefix: prefix.clone() });
        assert_eq!(Install::detect_for(Path::new("/usr/bin/kairo"), &none), Install::Other);
        let list = dir.path().join("kairo.list");
        std::fs::write(&list, "/.\n/usr/bin/kairo\n").unwrap();
        assert_eq!(Install::detect_for(Path::new("/usr/bin/kairo"), &list), Install::Deb);
        assert_eq!(Install::detect_for(Path::new("/home/x/target/debug/kairo"), &none), Install::Other);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_tarball_replaces_the_prefix_install() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("p");
        for (f, body) in [("bin/kairo", "old"), ("lib/kairo/mihomo", "old core"), ("share/applications/dev.kairo.app.desktop", "Exec=/p/bin/kairo")] {
            std::fs::create_dir_all(prefix.join(f).parent().unwrap()).unwrap();
            std::fs::write(prefix.join(f), body).unwrap();
        }
        // A release tarball.
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (f, body) in [
            ("Kairo-0.1.3-linux-x86_64/bin/kairo", "new"),
            ("Kairo-0.1.3-linux-x86_64/lib/kairo/mihomo", "new core"),
            ("Kairo-0.1.3-linux-x86_64/share/doc/kairo/LICENSE.mihomo", "GPL"),
            ("Kairo-0.1.3-linux-x86_64/share/applications/dev.kairo.app.desktop", "Exec=kairo"),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            tar.append_data(&mut h, f, body.as_bytes()).unwrap();
        }
        let gz = tar.into_inner().unwrap().finish().unwrap();
        let exe = install_prefix(&prefix, &gz, &dir.path().join("work")).unwrap();
        assert_eq!(exe, prefix.join("bin/kairo"));
        let read = |f: &str| std::fs::read_to_string(prefix.join(f)).unwrap();
        assert_eq!(read("bin/kairo"), "new");
        assert_eq!(read("lib/kairo/mihomo"), "new core");
        assert_eq!(read("share/doc/kairo/LICENSE.mihomo"), "GPL");
        assert_eq!(read("share/applications/dev.kairo.app.desktop"), "Exec=/p/bin/kairo", "the entry keeps its prefix");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(prefix.join("bin/kairo")).unwrap().permissions().mode() & 0o111, 0o111);
    }
}
