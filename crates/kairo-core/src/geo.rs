//! The geo data a config's rules need (`geoip.metadb` for GEOIP rules and the
//! DNS fallback filter, `GeoSite.dat` for GEOSITE rules). The core downloads a
//! missing file itself, but only while it loads the config: before it answers
//! at all, and from GitHub, which can take minutes (or fail with a 504 page the
//! core then keeps). So Kairo fetches them ahead, in the background from the
//! first launch, from whichever mirror answers first, and Connect finds them
//! in place.

use std::path::Path;
#[cfg(feature = "engine")]
use std::time::Duration;

use crate::paths::Paths;
#[cfg(feature = "engine")]
use crate::paths::write_atomic;

/// A file the core reads from its working directory, and where it comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeoFile {
    pub name: &'static str,
    source: &'static str,
}

pub const MMDB: GeoFile = GeoFile {
    name: "geoip.metadb",
    source: "geoip.metadb",
};
pub const GEOSITE: GeoFile = GeoFile {
    name: "GeoSite.dat",
    source: "geosite.dat",
};

/// MetaCubeX's releases: jsDelivr's mirrors first (they answer where GitHub
/// is slow), GitHub itself last.
#[cfg(feature = "engine")]
const MIRRORS: [&str; 4] = [
    "https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release",
    "https://fastly.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release",
    "https://cdn.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release",
    "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest",
];

/// Smaller than this is not the data (a 504 page is 92 bytes).
const MIN_SIZE: u64 = 512 * 1024;

/// Whether `file` is in the core's working directory and looks whole.
pub fn present(paths: &Paths, file: GeoFile) -> bool {
    usable(&paths.runtime_dir().join(file.name))
}

fn usable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() >= MIN_SIZE)
}

/// The files a config's text asks for.
pub fn needed(config: &str) -> Vec<GeoFile> {
    let mut out = Vec::new();
    // GEOIP rules, and the DNS fallback filter's geoip (on unless turned off),
    // read the MMDB; nearly every subscription has one or the other.
    if config.contains("GEOIP,") || config.contains("fallback") || config.contains("geoip") {
        out.push(MMDB);
    }
    if config.contains("GEOSITE,") || config.contains("geosite:") {
        out.push(GEOSITE);
    }
    out
}

#[cfg(feature = "engine")]
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("Kairo/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())
}

/// Puts `file` in place unless it already is: asks every mirror at once, takes
/// the first that answers, and falls back to the next if it fails midway.
/// `Ok(true)` when it downloaded the file.
#[cfg(feature = "engine")]
pub async fn fetch(paths: &Paths, file: GeoFile) -> Result<bool, String> {
    let target = paths.runtime_dir().join(file.name);
    if usable(&target) {
        return Ok(false);
    }
    let client = client()?;
    let mut pending: Vec<_> = MIRRORS
        .iter()
        .map(|base| {
            let url = format!("{base}/{}", file.source);
            let req = client.get(url).timeout(Duration::from_secs(120)).send();
            Box::pin(async move {
                let resp = req.await.map_err(|e| e.to_string())?;
                if resp.status().is_success() {
                    Ok(resp)
                } else {
                    Err(format!("HTTP {}", resp.status().as_u16()))
                }
            })
        })
        .collect();
    let mut errors = Vec::new();
    while !pending.is_empty() {
        let (answer, _, rest) = futures_util::future::select_all(pending).await;
        pending = rest;
        let resp = match answer {
            Ok(r) => r,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let source = resp.url().host_str().unwrap_or("?").to_owned();
        match resp.bytes().await {
            Ok(bytes) if bytes.len() as u64 >= MIN_SIZE => {
                std::fs::create_dir_all(paths.runtime_dir()).map_err(|e| e.to_string())?;
                write_atomic(&target, &bytes).map_err(|e| e.to_string())?;
                log::info!("{} ({} bytes) from {source}", file.name, bytes.len());
                return Ok(true);
            }
            Ok(bytes) => errors.push(format!("{source}: only {} bytes", bytes.len())),
            Err(e) => errors.push(format!("{source}: {e}")),
        }
    }
    Err(format!("{}: {}", file.name, errors.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_what_a_config_needs() {
        assert_eq!(needed("rules:\n  - GEOIP,CN,DIRECT\n"), vec![MMDB]);
        assert_eq!(
            needed("rules:\n  - GEOSITE,cn,DIRECT\n  - GEOIP,CN,DIRECT\n"),
            vec![MMDB, GEOSITE]
        );
        assert!(needed("rules:\n  - MATCH,DIRECT\n").is_empty());
    }

    #[test]
    fn a_short_file_is_not_the_data() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(paths.runtime_dir()).unwrap();
        let f = paths.runtime_dir().join(MMDB.name);
        std::fs::write(
            &f,
            b"<html><body><h1>504 Gateway Time-out</h1></body></html>",
        )
        .unwrap();
        assert!(!present(&paths, MMDB));
        std::fs::write(&f, vec![0u8; MIN_SIZE as usize]).unwrap();
        assert!(present(&paths, MMDB));
    }
}
