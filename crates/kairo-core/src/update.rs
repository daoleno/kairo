//! Kairo updates itself, the bundled mihomo core included (one updater).
//!
//! Every release carries `kairo-update.json` (the version and, per package,
//! its file name and SHA-256) and `kairo-update.json.sig`, an Ed25519
//! signature of the manifest's exact bytes made in the release workflow. The
//! public key is compiled in below. Kairo:
//!
//! 1. fetches the latest manifest (on launch, then every few hours) and
//!    checks its signature; if it names a newer version, the state is
//!    [`UpdateStatus::Available`] and nothing more happens by itself;
//! 2. on "Download" ([`Service::download_update`]), downloads this
//!    installation's package and checks its SHA-256 against the signed
//!    manifest and the release's `SHA256SUMS`;
//! 3. keeps it under `<home>/update/` and offers "Restart to Update". That
//!    tap installs it and starts the new Kairo.
//!
//! How it installs depends on how Kairo was installed ([`Install`]): the macOS
//! app bundle is swapped for the new one; on Windows the program's folder (installed per user, or
//! unzipped where the user can write) has its files renamed aside and replaced; a tarball install (`install.sh`,
//! `~/.local`) has its files replaced in place; the .deb goes through
//! `pkexec apt-get install` (one password prompt). Anything else gets a link to
//! the release.
//!
//! `KAIRO_UPDATE_URL` points the updater at another place that serves the same
//! files (tests). The key cannot be changed at run time. `KAIRO_NO_UPDATE=1` turns the
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
const PUBLIC_KEY: [u8; 32] = hex_literal(trusted_key_hex());

/// The key the build trusts: the release key, unless the build itself says
/// otherwise (`KAIRO_TEST_UPDATE_KEY_HEX` when compiling, for the builds that
/// test the updater against a manifest signed with a throwaway key). Nothing
/// at run time can change it.
const fn trusted_key_hex() -> &'static str {
    match option_env!("KAIRO_TEST_UPDATE_KEY_HEX") {
        Some(test) => test,
        None => UPDATE_PUBLIC_KEY_HEX,
    }
}
// One line: the release workflow reads it with sed.
#[rustfmt::skip]
const UPDATE_PUBLIC_KEY_HEX: &str = "336a3afab474faf88ed05e20cfe862fde8e0ad0f65abbc0e9fc88a222add86bf";

const LATEST: &str = "https://github.com/daoleno/kairo/releases/latest/download";
const RELEASES: &str = "https://github.com/daoleno/kairo/releases";
pub const MANIFEST: &str = "kairo-update.json";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const RETRY_FIRST: Duration = Duration::from_secs(10 * 60);

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
    UpToDate {
        checked: String,
    },
    /// A newer Kairo exists (its manifest is signed and checked); nothing is
    /// downloaded until [`Service::download_update`] is asked.
    Available {
        version: String,
    },
    Downloading {
        version: String,
        progress: Option<f32>,
    },
    /// Downloaded and verified: one tap installs it.
    Ready {
        version: String,
    },
    /// This installation cannot update itself: the release page.
    Manual {
        version: String,
        url: String,
    },
    Installing {
        version: String,
    },
    Failed {
        message: String,
    },
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
    /// `kairo.exe` next to `mihomo.exe` in a folder the user can write (Windows).
    WindowsDir { dir: PathBuf },
    /// `/usr/bin/kairo` from the .deb.
    Deb,
    /// Anything else (a development build, a system-wide copy made with sudo).
    Other,
}

impl Install {
    pub fn detect() -> Install {
        let Some(exe) = crate::paths::current_exe() else {
            return Install::Other;
        };
        if cfg!(windows) {
            return Self::detect_windows(&exe);
        }
        Self::detect_for(&exe, Path::new("/var/lib/dpkg/info/kairo.list"))
    }

    pub fn detect_for(exe: &Path, dpkg_list: &Path) -> Install {
        if cfg!(target_os = "macos") {
            let bundle = exe.parent().and_then(Path::parent).and_then(Path::parent);
            if let Some(bundle) = bundle
                && bundle.extension().is_some_and(|e| e == "app")
                && exe.parent().is_some_and(|d| d.ends_with("Contents/MacOS"))
            {
                return Install::MacApp {
                    bundle: bundle.to_owned(),
                };
            }
            return Install::Other;
        }
        if exe == Path::new("/usr/bin/kairo") {
            let owned = std::fs::read_to_string(dpkg_list)
                .is_ok_and(|l| l.lines().any(|f| f == "/usr/bin/kairo"));
            return if owned { Install::Deb } else { Install::Other };
        }
        let Some(bin) = exe.parent() else {
            return Install::Other;
        };
        let Some(prefix) = bin.parent() else {
            return Install::Other;
        };
        if bin.file_name().is_some_and(|n| n == "bin")
            && prefix.join("lib/kairo/mihomo").is_file()
            && writable(bin)
            && writable(&prefix.join("lib/kairo"))
        {
            return Install::Prefix {
                prefix: prefix.to_owned(),
            };
        }
        Install::Other
    }

    /// A Windows install: the folder of `kairo.exe` holds the core and can be
    /// written to by the user (per-user install, or a portable unzip).
    pub fn detect_windows(exe: &Path) -> Install {
        let Some(dir) = exe.parent() else {
            return Install::Other;
        };
        if dir.join("mihomo.exe").is_file() && writable(dir) {
            Install::WindowsDir {
                dir: dir.to_owned(),
            }
        } else {
            Install::Other
        }
    }

    /// The manifest's key for this installation's package.
    pub fn package_key(&self) -> Option<String> {
        let arch = std::env::consts::ARCH;
        match self {
            Install::MacApp { .. } => Some("macos-universal-zip".into()),
            Install::Prefix { .. } => Some(format!("linux-{arch}-tar")),
            Install::Deb => Some(format!("linux-{arch}-deb")),
            Install::WindowsDir { .. } => Some(format!("windows-{arch}-zip")),
            Install::Other => None,
        }
    }
}

fn writable(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()) else {
            return false;
        };
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        // Try it: a folder under Program Files refuses a standard user.
        let probe = dir.join(format!(".kairo-write-test-{}", std::process::id()));
        let ok = std::fs::write(&probe, b"").is_ok();
        let _ = std::fs::remove_file(&probe);
        ok
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
    key.verify_strict(bytes, &Signature::from_bytes(&sig))
        .map_err(|_| VerifyError::Signature)?;
    serde_json::from_slice(bytes).map_err(|e| VerifyError::Manifest(e.to_string()))
}

/// Checks `bytes` against the signed SHA-256 and the release's `SHA256SUMS`.
pub fn verify_package(bytes: &[u8], package: &Package, sums: &str) -> Result<(), VerifyError> {
    let actual = hex::encode(Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(&package.sha256) {
        return Err(VerifyError::Checksum(format!(
            "expected {}, got {actual}",
            package.sha256
        )));
    }
    let listed = sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (sum, name) = (it.next()?, it.next()?);
        (name.trim_start_matches('*') == package.name).then(|| sum.to_owned())
    });
    match listed {
        Some(s) if s.eq_ignore_ascii_case(&actual) => Ok(()),
        Some(s) => Err(VerifyError::Checksum(format!(
            "SHA256SUMS says {s}, got {actual}"
        ))),
        None => Err(VerifyError::Checksum(format!(
            "{} is not in SHA256SUMS",
            package.name
        ))),
    }
}

fn base_url() -> String {
    std::env::var("KAIRO_UPDATE_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| LATEST.into())
}

/// Where a release's files are.
fn file_url(version: &str, name: &str) -> String {
    match std::env::var("KAIRO_UPDATE_URL")
        .ok()
        .filter(|u| !u.is_empty())
    {
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

async fn get(
    url: &str,
    proxy: Option<&str>,
    mut progress: impl FnMut(u64, u64),
) -> Result<Vec<u8>, String> {
    // GitHub's download servers answer 5xx now and then: try again a few times.
    let mut attempt = 0;
    let resp = loop {
        attempt += 1;
        let r = client(proxy)?
            .get(url)
            .timeout(Duration::from_secs(900))
            .send()
            .await
            .map_err(|e| format!("Could not reach {}: {e}", host(url)));
        match r {
            Ok(resp) if resp.status().is_server_error() && attempt < 4 => {}
            Ok(resp) if resp.status().is_success() => break resp,
            Ok(resp) => {
                return Err(format!(
                    "{} answered HTTP {}",
                    host(url),
                    resp.status().as_u16()
                ));
            }
            Err(e) if attempt >= 4 => return Err(e),
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(2 * attempt)).await;
    };
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
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned())
}

/// What is waiting in `<home>/update/`.
#[derive(Debug, Clone, serde::Serialize, Deserialize, PartialEq)]
pub(crate) struct Staged {
    version: String,
    file: String,
    sha256: String,
}

fn staged_info(dir: &Path) -> Option<Staged> {
    std::fs::read(dir.join("staged.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
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
        // After a failed check, try again soon (10, 20, 40 … minutes, up to
        // the usual interval) rather than hours later.
        let mut retry = RETRY_FIRST;
        loop {
            let wait = match check(&svc).await {
                Ok(_) => {
                    retry = RETRY_FIRST;
                    CHECK_EVERY
                }
                Err(e) => {
                    log::warn!("update check: {e}");
                    let wait = retry;
                    retry = (retry * 2).min(CHECK_EVERY);
                    wait
                }
            };
            tokio::time::sleep(wait).await;
        }
    });
}

/// What a verified manifest means for this installation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Found {
    /// Nothing newer than this Kairo.
    Nothing,
    /// A newer one, already downloaded and verified.
    Staged(String),
    /// A newer one this installation can download and install by itself.
    Downloadable(String),
    /// A newer one this installation cannot install by itself.
    Manual { version: String, url: String },
}

impl Found {
    fn version(&self) -> Option<&str> {
        match self {
            Found::Nothing => None,
            Found::Staged(v) | Found::Downloadable(v) => Some(v),
            Found::Manual { version, .. } => Some(version),
        }
    }
}

/// Reads a manifest: whether it is newer, whether its package is already on
/// disk, and whether this installation can install it. Pure.
pub(crate) fn classify(
    manifest: &Manifest,
    current: &str,
    staged: Option<&Staged>,
    staged_file_present: bool,
    package: Option<&Package>,
) -> Found {
    let version = &manifest.version;
    if !is_newer(version, current) {
        return Found::Nothing;
    }
    if staged_file_present && staged.is_some_and(|s| &s.version == version) {
        return Found::Staged(version.clone());
    }
    if package.is_none() {
        return Found::Manual {
            version: version.clone(),
            url: format!("{RELEASES}/tag/v{version}"),
        };
    }
    Found::Downloadable(version.clone())
}

/// The status after a check that `found` something, given the one before.
/// What is already on offer at that version (or a newer one) is not flipped
/// back or restarted; a download in progress is never interrupted. A newer
/// version than the one downloaded replaces `Ready` with `Available` (the
/// older file stays on disk until the new download replaces it).
pub(crate) fn after_check(previous: &UpdateStatus, found: Found, checked: &str) -> UpdateStatus {
    use UpdateStatus::*;
    if matches!(previous, Installing { .. } | Downloading { .. }) {
        return previous.clone();
    }
    // Already offering `v` or something newer.
    let offered = |v: &str| match previous {
        Available { version } | Manual { version, .. } => !is_newer(v, version),
        // A download of a newer version than the manifest names (a release
        // taken back): kept. The same version with no file left is not.
        Ready { version } => is_newer(version, v),
        _ => false,
    };
    match found {
        Found::Nothing => match previous {
            // The download stays offered even if the manifest moved on.
            Ready { .. } => previous.clone(),
            _ => UpToDate {
                checked: checked.to_owned(),
            },
        },
        Found::Staged(version) => Ready { version },
        Found::Downloadable(v) if offered(&v) => previous.clone(),
        Found::Downloadable(version) => Available { version },
        Found::Manual { version, .. } if offered(&version) => previous.clone(),
        Found::Manual { version, url } => Manual { version, url },
    }
}

/// Whether a check shows no "Checking…" over this status: something is
/// already on offer, and a check only looks for something newer.
fn quiet(status: &UpdateStatus) -> bool {
    matches!(
        status,
        UpdateStatus::Ready { .. }
            | UpdateStatus::Available { .. }
            | UpdateStatus::Manual { .. }
            | UpdateStatus::Downloading { .. }
    )
}

/// Looks for a newer Kairo (the signed manifest only: nothing is downloaded;
/// see [`download`]). `Ok(Some(version))` when one is on offer, `Ok(None)`
/// when up to date.
pub async fn check(svc: &Service) -> Result<Option<String>, String> {
    let _busy = match svc.update_lock().try_lock() {
        Ok(g) => g,
        Err(_) => return Ok(None),
    };
    let previous = svc.snapshot().update;
    if matches!(previous, UpdateStatus::Installing { .. }) {
        return Ok(None);
    }
    let silent = quiet(&previous);
    if !silent {
        svc.set_update(UpdateStatus::Checking);
    }
    let result = check_inner(svc).await;
    match &result {
        Ok(found) => {
            let checked = crate::service::clock()[..5].to_owned();
            svc.set_update(after_check(&previous, found.clone(), &checked));
        }
        Err(e) if silent => {
            log::warn!("update: {e}");
            svc.set_update(previous);
        }
        Err(e) => svc.set_update(UpdateStatus::Failed { message: e.clone() }),
    }
    result.map(|f| f.version().map(str::to_owned))
}

async fn check_inner(svc: &Service) -> Result<Found, String> {
    let routes = svc.download_routes();
    let mut last_err = String::new();
    for proxy in &routes {
        match fetch_manifest(proxy.as_deref()).await {
            Ok(m) => {
                let dir = svc.paths().update_dir();
                let staged = staged_info(&dir);
                let present = staged.as_ref().is_some_and(|s| dir.join(&s.file).is_file());
                let package = package_of(&m);
                return Ok(classify(
                    &m,
                    VERSION,
                    staged.as_ref(),
                    present,
                    package.as_ref(),
                ));
            }
            Err(Fetch::Network(e)) => last_err = e,
            Err(Fetch::Fatal(e)) => return Err(e),
        }
    }
    Err(last_err)
}

/// This installation's package in a manifest.
fn package_of(manifest: &Manifest) -> Option<Package> {
    Install::detect()
        .package_key()
        .and_then(|k| manifest.packages.get(&k).cloned())
}

enum Fetch {
    /// Worth trying another route.
    Network(String),
    Fatal(String),
}

/// The latest manifest, its signature checked.
async fn fetch_manifest(proxy: Option<&str>) -> Result<Manifest, Fetch> {
    let base = base_url();
    let base = base.trim_end_matches('/');
    let bytes = get(&format!("{base}/{MANIFEST}"), proxy, |_, _| {})
        .await
        .map_err(Fetch::Network)?;
    let sig = get(&format!("{base}/{MANIFEST}.sig"), proxy, |_, _| {})
        .await
        .map_err(Fetch::Network)?;
    verify_manifest(&bytes, &String::from_utf8_lossy(&sig)).map_err(|e| Fetch::Fatal(e.to_string()))
}

/// Downloads the update on offer ([`UpdateStatus::Available`]): fetches the
/// manifest again (signed), then the package, checks it, and keeps it. On
/// failure the status goes back to `Available`, the person is told in plain
/// words (a notice) and the call returns `Ok`: the state and the notice are
/// the report, so a caller that shows errors itself does not say it twice.
/// Asked again while a download runs or is done, it does nothing; asked
/// while a check runs, it waits for the check.
pub async fn download(svc: &Service) -> Result<(), String> {
    // What is on offer, if a download is wanted at all.
    fn wanted(status: UpdateStatus) -> Result<Option<String>, String> {
        match status {
            UpdateStatus::Available { version } => Ok(Some(version)),
            UpdateStatus::Ready { .. } | UpdateStatus::Downloading { .. } => Ok(None),
            UpdateStatus::Manual { .. } => {
                Err("This copy of Kairo cannot update itself: use the release page".into())
            }
            _ => Err("No update is waiting to be downloaded".into()),
        }
    }
    if wanted(svc.snapshot().update)?.is_none() {
        return Ok(());
    }
    // A check may be running (it holds the lock for as long as the network
    // takes): wait for it rather than drop the person's click, then look
    // again, since it may have found something else or nothing. Only now is
    // the status changed: a check would otherwise write over it.
    let _busy = svc.update_lock().lock().await;
    let Some(version) = wanted(svc.snapshot().update)? else {
        return Ok(());
    };
    svc.set_update(UpdateStatus::Downloading {
        version: version.clone(),
        progress: None,
    });
    match download_inner(svc).await {
        Ok(ready) => svc.set_update(UpdateStatus::Ready { version: ready }),
        Err(e) => {
            log::warn!("update download: {}", e.detail());
            svc.set_update(UpdateStatus::Available {
                version: version.clone(),
            });
            svc.notify(failure_words(&version, &e), true);
        }
    }
    Ok(())
}

#[derive(Debug)]

enum DownloadError {
    Network(String),
    Verify(String),
}

impl DownloadError {
    /// What went wrong, for the log.
    fn detail(&self) -> &str {
        match self {
            DownloadError::Network(m) | DownloadError::Verify(m) => m,
        }
    }
}

/// What the person reads when a download did not work out.
fn failure_words(version: &str, why: &DownloadError) -> String {
    match why {
        DownloadError::Network(_) => {
            format!("Kairo {version} could not be downloaded. Check the connection and try again.")
        }
        DownloadError::Verify(_) => {
            format!("Kairo {version} did not download correctly and was thrown away. Try again.")
        }
    }
}

async fn download_inner(svc: &Service) -> Result<String, DownloadError> {
    let routes = svc.download_routes();
    let mut last_err = String::new();
    for proxy in &routes {
        match stage(svc, proxy.as_deref()).await {
            Ok(v) => return Ok(v),
            Err(Fetch::Network(e)) => last_err = e,
            Err(Fetch::Fatal(e)) => return Err(DownloadError::Verify(e)),
        }
    }
    Err(DownloadError::Network(last_err))
}

/// Fetches, checks and keeps the package of the latest manifest.
async fn stage(svc: &Service, proxy: Option<&str>) -> Result<String, Fetch> {
    let manifest = fetch_manifest(proxy).await?;
    if !is_newer(&manifest.version, VERSION) {
        return Err(Fetch::Fatal("The update is no longer on offer".to_owned()));
    }
    let version = manifest.version.clone();
    let dir = svc.paths().update_dir();
    if staged_info(&dir).is_some_and(|s| s.version == version && dir.join(&s.file).is_file()) {
        return Ok(version);
    }
    let package = package_of(&manifest)
        .ok_or_else(|| Fetch::Fatal("This copy of Kairo cannot update itself".to_owned()))?;
    svc.set_update(UpdateStatus::Downloading {
        version: version.clone(),
        progress: None,
    });
    let sums = get(&file_url(&version, "SHA256SUMS"), proxy, |_, _| {})
        .await
        .map_err(Fetch::Network)?;
    let progress_svc = svc.clone();
    let progress_version = version.clone();
    let mut last = 0.0f32;
    let file = get(
        &file_url(&version, &package.name),
        proxy,
        move |done, total| {
            let p = if total > 0 {
                done as f32 / total as f32
            } else {
                0.0
            };
            if p - last >= 0.02 || p >= 1.0 {
                last = p;
                progress_svc.set_update(UpdateStatus::Downloading {
                    version: progress_version.clone(),
                    progress: Some(p),
                });
            }
        },
    )
    .await
    .map_err(Fetch::Network)?;
    verify_package(&file, &package, &String::from_utf8_lossy(&sums))
        .map_err(|e| Fetch::Fatal(e.to_string()))?;
    let _ = std::fs::remove_dir_all(&dir);
    let io = |e: std::io::Error| Fetch::Fatal(format!("Could not keep the update: {e}"));
    std::fs::create_dir_all(&dir).map_err(io)?;
    write_atomic(&dir.join(&package.name), &file).map_err(io)?;
    let staged = Staged {
        version: version.clone(),
        file: package.name.clone(),
        sha256: package.sha256.clone(),
    };
    write_atomic(
        &dir.join("staged.json"),
        &serde_json::to_vec_pretty(&staged).expect("json"),
    )
    .map_err(io)?;
    log::info!("Kairo {version} downloaded and verified");
    Ok(version)
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
        Install::WindowsDir { dir } => install_windows(&dir, &bytes, &work),
        Install::Other => Err("This copy of Kairo cannot update itself".into()),
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(dir.join("work"));
    result
}

#[cfg(target_os = "macos")]
fn run(cmd: &mut std::process::Command) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
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
        run(Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(zip)
            .arg(&stage))
        .map_err(cleanup)?;
        let new = stage.join("Kairo.app");
        run(Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&new))
        .map_err(cleanup)?;
        if !new.join("Contents/MacOS/Kairo").is_file()
            || !new.join("Contents/MacOS/mihomo").is_file()
        {
            return Err(cleanup("The update is missing files".into()));
        }
        let old = parent.join(format!(".Kairo-old-{pid}.app"));
        std::fs::rename(bundle, &old)
            .map_err(|e| cleanup(format!("Could not move the old Kairo aside: {e}")))?;
        if let Err(e) = std::fs::rename(&new, bundle) {
            let _ = std::fs::rename(&old, bundle);
            return Err(cleanup(format!(
                "Could not put the new Kairo in place: {e}"
            )));
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
    archive
        .unpack(work)
        .map_err(|e| format!("Could not unpack the update: {e}"))?;
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
    let tmp = dir.join(format!(
        ".{}.update",
        to.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    std::fs::copy(from, &tmp).map_err(|e| format!("{}: {e}", to.display()))?;
    std::fs::rename(&tmp, to).map_err(|e| format!("{}: {e}", to.display()))
}

/// Unpacks the zip and replaces the program's files with renames: Windows will
/// not overwrite a running executable, but it will rename one, and the new file
/// takes its name. The old files stay as `<name>.old` until the next start
/// ([`remove_leftovers`]). If anything fails, what was moved aside comes back.
pub fn install_windows(dir: &Path, zip_bytes: &[u8], work: &Path) -> Result<PathBuf, String> {
    use std::io::{Cursor, Read};
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))
        .map_err(|e| format!("Could not read the update: {e}"))?;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("Could not read the update: {e}"))?;
        // Names that climb out of the folder are refused.
        let Some(rel) = file.enclosed_name() else {
            return Err("The update has a file outside its folder".into());
        };
        let target = work.join(rel);
        if file.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
    }
    // The package is a folder of files (maybe inside one folder).
    let top = if work.join("kairo.exe").is_file() {
        work.to_owned()
    } else {
        std::fs::read_dir(work)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.join("kairo.exe").is_file())
            .ok_or("The update has no kairo.exe")?
    };
    if !top.join("mihomo.exe").is_file() {
        return Err("The update has no core".into());
    }
    let mut files = Vec::new();
    collect_files(&top, &top, &mut files).map_err(|e| e.to_string())?;
    // The launcher last: if anything before fails, the old one still runs.
    files.sort();
    files.sort_by_key(|f| f == Path::new("kairo.exe"));
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for rel in &files {
        if let Err(e) = replace_windows_file(&top.join(rel), &dir.join(rel), &mut moved) {
            for (old, original) in moved.iter().rev() {
                let _ = std::fs::remove_file(original);
                let _ = std::fs::rename(old, original);
            }
            return Err(e);
        }
    }
    Ok(dir.join("kairo.exe"))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_owned());
        }
    }
    Ok(())
}

/// Moves `to` aside (`to.old`) and puts a copy of `from` in its place.
fn replace_windows_file(
    from: &Path,
    to: &Path,
    moved: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if to.exists() {
        let mut old = PathBuf::from(format!("{}.old", to.display()));
        // A leftover of an earlier update that is still in use: pick another name.
        let mut n = 1;
        while old.exists() && std::fs::remove_file(&old).is_err() {
            old = PathBuf::from(format!("{}.old{n}", to.display()));
            n += 1;
        }
        std::fs::rename(to, &old).map_err(|e| format!("{}: {e}", to.display()))?;
        moved.push((old, to.to_owned()));
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", to.display()))
}

/// Deletes what an update left beside the program (`*.old`): called at start,
/// when the old executable is no longer running. What is still in use stays.
pub fn remove_leftovers(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_old = name
            .rsplit_once(".old")
            .is_some_and(|(_, tail)| tail.chars().all(|c| c.is_ascii_digit()));
        if is_old && path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `pkexec apt-get install <deb>`: one password prompt, dependencies resolved.
#[allow(unused_variables)]
fn install_deb(deb: &Path) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let apt = ["/usr/bin/apt-get", "/usr/bin/apt"]
            .into_iter()
            .find(|p| Path::new(p).exists());
        let mut cmd = Command::new("pkexec");
        match apt {
            Some(apt) => cmd
                .args([apt, "install", "-y", "--no-install-recommends"])
                .arg(deb),
            None => cmd.args(["/usr/bin/dpkg", "-i"]).arg(deb),
        };
        cmd.env("DEBIAN_FRONTEND", "noninteractive");
        let out = cmd.output().map_err(|e| format!("pkexec: {e}"))?;
        return match out.status.code() {
            Some(0) => Ok(PathBuf::from("/usr/bin/kairo")),
            Some(126) => Err("Cancelled".into()),
            Some(127) => Err("No polkit agent could ask for the password".into()),
            _ => Err(String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("apt-get failed")
                .to_owned()),
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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: it outlives this one.
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not start the new Kairo: {e}"))
}

/// In a relaunched Kairo: waits (up to 30 s) until the process it replaces is gone.
pub fn wait_for_previous() {
    let args: Vec<String> = std::env::args().collect();
    let Some(pid) = args
        .iter()
        .position(|a| a == AFTER_FLAG)
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse::<i32>().ok())
    else {
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
            && let Some(state) = stat
                .rsplit(')')
                .next()
                .and_then(|r| r.split_whitespace().next())
        {
            return state != "Z";
        }
        true
    }
    #[cfg(windows)]
    {
        crate::winapi::pid_alive(pid as u32)
    }
    #[cfg(not(any(unix, windows)))]
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
        assert_eq!(
            m.packages["linux-x86_64-tar"].name,
            "Kairo-0.1.3-linux-x86_64.tar.gz"
        );
        // A changed byte, another key, garbage: refused.
        let tampered = JSON.replace("0.1.3\",", "0.1.4\",");
        assert_eq!(
            verify_with(&public, tampered.as_bytes(), &sig),
            Err(VerifyError::Signature)
        );
        let other = SigningKey::from_bytes(&[8u8; 32]);
        assert_eq!(
            verify_with(&public, JSON.as_bytes(), &signed(&other, JSON)),
            Err(VerifyError::Signature)
        );
        assert_eq!(
            verify_with(&public, JSON.as_bytes(), "zz"),
            Err(VerifyError::Signature)
        );
        // The release key does not accept a test signature.
        assert_eq!(
            verify_manifest(JSON.as_bytes(), &sig),
            Err(VerifyError::Signature)
        );
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
        assert_eq!(
            verify_with(&public, JSON.as_bytes(), &sig_hex)
                .unwrap()
                .version,
            "0.1.3"
        );
    }

    #[test]
    fn packages_are_checked_against_both_sums() {
        let bytes = b"package";
        let sum = hex::encode(Sha256::digest(bytes));
        let p = Package {
            name: "k.tar.gz".into(),
            sha256: sum.clone(),
        };
        assert!(verify_package(bytes, &p, &format!("{sum}  k.tar.gz\nff  other\n")).is_ok());
        assert!(verify_package(b"other", &p, &format!("{sum}  k.tar.gz\n")).is_err());
        assert!(verify_package(bytes, &p, "ff  k.tar.gz\n").is_err());
        assert!(verify_package(bytes, &p, "").is_err());
    }

    fn manifest(version: &str) -> Manifest {
        Manifest {
            version: version.into(),
            packages: Default::default(),
        }
    }

    fn pkg() -> Package {
        Package {
            name: "k.tar.gz".into(),
            sha256: "ab".into(),
        }
    }

    fn staged(version: &str) -> Staged {
        Staged {
            version: version.into(),
            file: "k.tar.gz".into(),
            sha256: "ab".into(),
        }
    }

    #[test]
    fn a_manifest_is_classified_without_any_download() {
        let p = pkg();
        // Not newer: nothing (the same version, or an older one).
        assert_eq!(
            classify(&manifest("0.2.0"), "0.2.0", None, false, Some(&p)),
            Found::Nothing
        );
        assert_eq!(
            classify(&manifest("0.1.9"), "0.2.0", None, false, Some(&p)),
            Found::Nothing
        );
        // Newer, can install itself: on offer, not downloaded.
        assert_eq!(
            classify(&manifest("0.3.0"), "0.2.0", None, false, Some(&p)),
            Found::Downloadable("0.3.0".into())
        );
        // Newer, cannot: the release page.
        assert_eq!(
            classify(&manifest("0.3.0"), "0.2.0", None, false, None),
            Found::Manual {
                version: "0.3.0".into(),
                url: format!("{RELEASES}/tag/v0.3.0"),
            }
        );
        // Newer and already on disk (and still there): ready.
        let s = staged("0.3.0");
        assert_eq!(
            classify(&manifest("0.3.0"), "0.2.0", Some(&s), true, Some(&p)),
            Found::Staged("0.3.0".into())
        );
        // The file is gone, or it is another version: offered again.
        assert_eq!(
            classify(&manifest("0.3.0"), "0.2.0", Some(&s), false, Some(&p)),
            Found::Downloadable("0.3.0".into())
        );
        assert_eq!(
            classify(&manifest("0.3.1"), "0.2.0", Some(&s), true, Some(&p)),
            Found::Downloadable("0.3.1".into())
        );
    }

    #[test]
    fn checks_move_the_status_only_forward() {
        use UpdateStatus::*;
        let avail = |v: &str| Available { version: v.into() };
        let ready = |v: &str| Ready { version: v.into() };
        let down = |v: &str| Found::Downloadable(v.into());
        let up_to_date = UpToDate {
            checked: "10:00".into(),
        };
        // First sight of a newer version.
        assert_eq!(after_check(&Idle, down("0.3.0"), "10:00"), avail("0.3.0"));
        assert_eq!(
            after_check(
                &UpToDate {
                    checked: "09:00".into()
                },
                down("0.3.0"),
                "10:00"
            ),
            avail("0.3.0")
        );
        // The same again: not flipped, not restarted.
        assert_eq!(
            after_check(&avail("0.3.0"), down("0.3.0"), "10:00"),
            avail("0.3.0")
        );
        assert_eq!(
            after_check(&ready("0.3.0"), Found::Staged("0.3.0".into()), "10:00"),
            ready("0.3.0")
        );
        // Nothing newer: what was downloaded stays offered; an offer that
        // vanished is not one any more.
        assert_eq!(
            after_check(&ready("0.3.0"), Found::Nothing, "10:00"),
            ready("0.3.0")
        );
        assert_eq!(
            after_check(&avail("0.3.0"), Found::Nothing, "10:00"),
            up_to_date
        );
        assert_eq!(after_check(&Idle, Found::Nothing, "10:00"), up_to_date);
        // A newer one than the one on offer or downloaded replaces it.
        assert_eq!(
            after_check(&avail("0.3.0"), down("0.3.1"), "10:00"),
            avail("0.3.1")
        );
        assert_eq!(
            after_check(&ready("0.3.0"), down("0.3.1"), "10:00"),
            avail("0.3.1")
        );
        // An older manifest (a re-published release) never takes the place of a newer offer.
        assert_eq!(
            after_check(&avail("0.3.1"), down("0.3.0"), "10:00"),
            avail("0.3.1")
        );
        // A download in progress, or an install, is never touched.
        let busy = Downloading {
            version: "0.3.0".into(),
            progress: Some(0.4),
        };
        assert_eq!(after_check(&busy, down("0.3.0"), "10:00"), busy);
        assert_eq!(after_check(&busy, Found::Nothing, "10:00"), busy);
        let installing = Installing {
            version: "0.3.0".into(),
        };
        assert_eq!(after_check(&installing, down("0.3.1"), "10:00"), installing);
        // A file left from an earlier run is found by the manifest: ready.
        assert_eq!(
            after_check(&avail("0.3.0"), Found::Staged("0.3.0".into()), "10:00"),
            ready("0.3.0")
        );
        // Manual stays one, and is not repeated.
        let manual = Manual {
            version: "0.3.0".into(),
            url: "u".into(),
        };
        assert_eq!(
            after_check(
                &manual,
                Found::Manual {
                    version: "0.3.0".into(),
                    url: "u".into()
                },
                "10:00"
            ),
            manual
        );
    }

    #[test]
    fn checks_over_an_offer_are_quiet() {
        use UpdateStatus::*;
        assert!(quiet(&Available {
            version: "1".into()
        }));
        assert!(quiet(&Ready {
            version: "1".into()
        }));
        assert!(quiet(&Downloading {
            version: "1".into(),
            progress: None
        }));
        assert!(!quiet(&Idle));
        assert!(!quiet(&UpToDate {
            checked: "10:00".into()
        }));
        assert!(!quiet(&Failed {
            message: "x".into()
        }));
    }

    #[test]
    fn a_failed_download_says_what_to_do_in_plain_words() {
        let net = failure_words("0.3.0", &DownloadError::Network("dns error".into()));
        assert!(net.contains("0.3.0") && net.contains("connection"));
        assert!(!net.contains("dns error"));
        let bad = failure_words("0.3.0", &DownloadError::Verify("expected ab".into()));
        assert!(bad.contains("thrown away") && !bad.contains("expected"));
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
        assert_eq!(
            Install::detect_for(&prefix.join("bin/kairo"), &none),
            Install::Other
        );
        std::fs::write(prefix.join("lib/kairo/mihomo"), "").unwrap();
        assert_eq!(
            Install::detect_for(&prefix.join("bin/kairo"), &none),
            Install::Prefix {
                prefix: prefix.clone()
            }
        );
        assert_eq!(
            Install::detect_for(Path::new("/usr/bin/kairo"), &none),
            Install::Other
        );
        let list = dir.path().join("kairo.list");
        std::fs::write(&list, "/.\n/usr/bin/kairo\n").unwrap();
        assert_eq!(
            Install::detect_for(Path::new("/usr/bin/kairo"), &list),
            Install::Deb
        );
        assert_eq!(
            Install::detect_for(Path::new("/home/x/target/debug/kairo"), &none),
            Install::Other
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_tarball_replaces_the_prefix_install() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("p");
        for (f, body) in [
            ("bin/kairo", "old"),
            ("lib/kairo/mihomo", "old core"),
            (
                "share/applications/dev.kairo.app.desktop",
                "Exec=/p/bin/kairo",
            ),
        ] {
            std::fs::create_dir_all(prefix.join(f).parent().unwrap()).unwrap();
            std::fs::write(prefix.join(f), body).unwrap();
        }
        // A release tarball.
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (f, body) in [
            ("Kairo-0.1.3-linux-x86_64/bin/kairo", "new"),
            ("Kairo-0.1.3-linux-x86_64/lib/kairo/mihomo", "new core"),
            (
                "Kairo-0.1.3-linux-x86_64/share/doc/kairo/LICENSE.mihomo",
                "GPL",
            ),
            (
                "Kairo-0.1.3-linux-x86_64/share/applications/dev.kairo.app.desktop",
                "Exec=kairo",
            ),
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
        assert_eq!(
            read("share/applications/dev.kairo.app.desktop"),
            "Exec=/p/bin/kairo",
            "the entry keeps its prefix"
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(prefix.join("bin/kairo"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0o111
        );
    }

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        let mut out = std::io::Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(&mut out);
        for (name, text) in files {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(text.as_bytes()).unwrap();
        }
        w.finish().unwrap();
        out.into_inner()
    }

    #[test]
    fn windows_update_renames_old_files_aside() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("app");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kairo.exe"), "old kairo").unwrap();
        std::fs::write(dir.join("mihomo.exe"), "old core").unwrap();
        let zip = zip_of(&[
            ("Kairo/kairo.exe", "new kairo"),
            ("Kairo/mihomo.exe", "new core"),
            ("Kairo/wintun.dll", "wintun"),
        ]);
        let exe = install_windows(&dir, &zip, &tmp.path().join("work")).unwrap();
        assert_eq!(exe, dir.join("kairo.exe"));
        assert_eq!(
            std::fs::read_to_string(dir.join("kairo.exe")).unwrap(),
            "new kairo"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("mihomo.exe")).unwrap(),
            "new core"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("wintun.dll")).unwrap(),
            "wintun"
        );
        // What a running program held is still there, under another name.
        assert_eq!(
            std::fs::read_to_string(dir.join("kairo.exe.old")).unwrap(),
            "old kairo"
        );
        remove_leftovers(&dir);
        assert!(!dir.join("kairo.exe.old").exists());
        assert!(!dir.join("mihomo.exe.old").exists());
        assert!(dir.join("kairo.exe").exists());
    }

    #[test]
    fn windows_update_without_a_core_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("app");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kairo.exe"), "old kairo").unwrap();
        let zip = zip_of(&[("kairo.exe", "new kairo")]);
        assert!(install_windows(&dir, &zip, &tmp.path().join("work")).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("kairo.exe")).unwrap(),
            "old kairo"
        );
    }

    #[test]
    fn windows_update_failing_halfway_puts_the_old_files_back() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("app");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kairo.exe"), "old kairo").unwrap();
        std::fs::write(dir.join("mihomo.exe"), "old core").unwrap();
        // A file where a folder must go: what is under it cannot be written.
        std::fs::write(dir.join("zz"), "in the way").unwrap();
        let zip = zip_of(&[
            ("kairo.exe", "new kairo"),
            ("mihomo.exe", "new core"),
            ("zz/readme.txt", "licence"),
        ]);
        assert!(install_windows(&dir, &zip, &tmp.path().join("work")).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("kairo.exe")).unwrap(),
            "old kairo"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("mihomo.exe")).unwrap(),
            "old core"
        );
    }

    #[test]
    fn windows_install_is_found_beside_its_core() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("kairo.exe");
        std::fs::write(&exe, "").unwrap();
        assert_eq!(Install::detect_windows(&exe), Install::Other);
        std::fs::write(tmp.path().join("mihomo.exe"), "").unwrap();
        let found = Install::detect_windows(&exe);
        assert_eq!(
            found,
            Install::WindowsDir {
                dir: tmp.path().to_owned()
            }
        );
        assert_eq!(
            found.package_key().unwrap().split('-').next(),
            Some("windows")
        );
    }
}
