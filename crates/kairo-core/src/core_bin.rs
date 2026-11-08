//! The mihomo core binary: finding the right official build for this machine,
//! downloading it with its SHA-256 checked against the release, and reading the
//! installed version.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths::{Paths, write_atomic};

const RELEASES: &str = "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest";

/// The installed core, as recorded when it was downloaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreMeta {
    pub version: String,
    pub asset: String,
    pub sha256: String,
    pub installed_at: i64,
}

/// A release asset to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub asset: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("No mihomo build for {0}")]
    Unsupported(String),
    #[error("Could not reach GitHub: {0}")]
    Network(String),
    #[error("The release has no {0}")]
    MissingAsset(String),
    #[error("The download's checksum does not match the release (expected {expected}, got {actual})")]
    Checksum { expected: String, actual: String },
    #[error("{0}")]
    Io(String),
}

/// The asset name prefix of the official build for an OS / arch (Rust's names).
pub fn asset_prefix(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("linux", "x86_64") => "mihomo-linux-amd64-compatible-",
        ("linux", "aarch64") => "mihomo-linux-arm64-",
        ("macos", "aarch64") => "mihomo-darwin-arm64-",
        ("macos", "x86_64") => "mihomo-darwin-amd64-compatible-",
        ("windows", "x86_64") => "mihomo-windows-amd64-compatible-",
        ("windows", "aarch64") => "mihomo-windows-arm64-",
        _ => return None,
    })
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    size: u64,
}

fn client(proxy: Option<&str>) -> Result<reqwest::Client, CoreError> {
    let mut b = reqwest::Client::builder()
        .user_agent(concat!("Kairo/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15));
    b = match proxy {
        Some(p) => b.proxy(reqwest::Proxy::all(p).map_err(|e| CoreError::Network(e.to_string()))?),
        None => b.no_proxy(),
    };
    b.build().map_err(|e| CoreError::Network(e.to_string()))
}

/// Picks this machine's asset out of a release listing (the JSON of GitHub's API).
fn pick(release: GhRelease, os: &str, arch: &str) -> Result<Release, CoreError> {
    let prefix = asset_prefix(os, arch).ok_or_else(|| CoreError::Unsupported(format!("{os} {arch}")))?;
    let ext = if os == "windows" { ".zip" } else { ".gz" };
    let name = format!("{prefix}{}{ext}", release.tag_name);
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name == name)
        .ok_or_else(|| CoreError::MissingAsset(name.clone()))?;
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(str::to_owned)
        .ok_or_else(|| CoreError::MissingAsset(format!("checksum for {name}")))?;
    Ok(Release {
        version: release.tag_name,
        asset: asset.name,
        url: asset.browser_download_url,
        sha256,
        size: asset.size,
    })
}

/// The latest official release's asset for this machine.
pub async fn latest(proxy: Option<&str>) -> Result<Release, CoreError> {
    let resp = client(proxy)?
        .get(RELEASES)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| CoreError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(CoreError::Network(format!("GitHub answered HTTP {}", resp.status().as_u16())));
    }
    let release: GhRelease = resp.json().await.map_err(|e| CoreError::Network(e.to_string()))?;
    pick(release, std::env::consts::OS, std::env::consts::ARCH)
}

/// Downloads `release`, checks its SHA-256, unpacks it and installs it as the
/// core. `progress` hears (bytes so far, total).
pub async fn install(
    paths: &Paths,
    release: &Release,
    proxy: Option<&str>,
    mut progress: impl FnMut(u64, u64),
) -> Result<CoreMeta, CoreError> {
    let resp = client(proxy)?
        .get(&release.url)
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| CoreError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(CoreError::Network(format!("HTTP {}", resp.status().as_u16())));
    }
    let total = resp.content_length().unwrap_or(release.size);
    let mut bytes = Vec::with_capacity(total as usize);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| CoreError::Network(e.to_string()))?;
        bytes.extend_from_slice(&chunk);
        progress(bytes.len() as u64, total);
    }
    let actual = hex::encode(Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(&release.sha256) {
        return Err(CoreError::Checksum { expected: release.sha256.clone(), actual });
    }
    let binary = unpack(&release.asset, &bytes)?;
    install_binary(paths, &binary)?;
    let meta = CoreMeta {
        version: release.version.clone(),
        asset: release.asset.clone(),
        sha256: actual,
        installed_at: crate::profile::now(),
    };
    write_atomic(&paths.core_meta(), &serde_json::to_vec_pretty(&meta).expect("meta"))
        .map_err(|e| CoreError::Io(e.to_string()))?;
    Ok(meta)
}

/// The binary inside a downloaded asset.
pub fn unpack(asset: &str, bytes: &[u8]) -> Result<Vec<u8>, CoreError> {
    if asset.ends_with(".gz") {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut out)
            .map_err(|e| CoreError::Io(format!("Could not unpack the core: {e}")))?;
        Ok(out)
    } else {
        Err(CoreError::Unsupported(format!("unpacking {asset}")))
    }
}

/// Writes the core binary and makes it executable.
pub fn install_binary(paths: &Paths, binary: &[u8]) -> Result<(), CoreError> {
    let target = paths.core_bin();
    write_atomic(&target, binary).map_err(|e| CoreError::Io(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| CoreError::Io(e.to_string()))?;
    }
    Ok(())
}

pub fn installed_meta(paths: &Paths) -> Option<CoreMeta> {
    std::fs::read(paths.core_meta()).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// The version a core binary reports (`mihomo -v`: "Mihomo Meta v1.19.33 linux amd64 ...").
pub async fn binary_version(core: &Path) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(core).arg("-v").kill_on_drop(true).output(),
    )
    .await
    .ok()?
    .ok()?;
    parse_version(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|w| w.starts_with('v') && w[1..].chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_owned)
        .or_else(|| text.split_whitespace().find(|w| w.starts_with("alpha")).map(str::to_owned))
}

/// Whether `latest` is newer than `installed` (semantic versions with a `v`).
pub fn is_newer(latest: &str, installed: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split(['.', '-'])
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parts(latest) > parts(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_asset_and_checksum() {
        let json = r#"{"tag_name":"v1.19.33","assets":[
          {"name":"mihomo-linux-amd64-v1.19.33.gz","browser_download_url":"u1","digest":"sha256:aa","size":1},
          {"name":"mihomo-linux-amd64-compatible-v1.19.33.gz","browser_download_url":"u2","digest":"sha256:bb","size":2},
          {"name":"mihomo-darwin-arm64-v1.19.33.gz","browser_download_url":"u3","digest":"sha256:cc","size":3}
        ]}"#;
        let r: GhRelease = serde_json::from_str(json).unwrap();
        let rel = pick(r, "linux", "x86_64").unwrap();
        assert_eq!((rel.url.as_str(), rel.sha256.as_str()), ("u2", "bb"));
        let r: GhRelease = serde_json::from_str(json).unwrap();
        assert_eq!(pick(r, "macos", "aarch64").unwrap().url, "u3");
        let r: GhRelease = serde_json::from_str(json).unwrap();
        assert!(matches!(pick(r, "linux", "aarch64"), Err(CoreError::MissingAsset(_))));
        let r: GhRelease = serde_json::from_str(json).unwrap();
        assert!(matches!(pick(r, "plan9", "x86_64"), Err(CoreError::Unsupported(_))));
    }

    #[test]
    fn versions() {
        assert_eq!(
            parse_version("Mihomo Meta v1.19.33 linux amd64 with go1.26.10 Wed Oct 21"),
            Some("v1.19.33".into())
        );
        assert!(is_newer("v1.19.33", "v1.19.18"));
        assert!(!is_newer("v1.19.33", "v1.19.33"));
        assert!(is_newer("v1.20.0", "v1.19.99"));
    }

    #[test]
    fn unpacks_gzip() {
        use std::io::Write;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"binary").unwrap();
        let gz = enc.finish().unwrap();
        assert_eq!(unpack("x.gz", &gz).unwrap(), b"binary");
    }
}
