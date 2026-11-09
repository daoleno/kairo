//! The mihomo core binary. Kairo ships one (see [`crate::paths::bundled_core`])
//! and runs a copy of it from its data directory, so that TUN's one-time grant
//! (file capabilities on Linux, setuid on macOS) is given to Kairo's own copy and
//! never to a file inside the app or a system package. The copy follows the
//! bundled core: when an app update brings another version, it is replaced.

use std::path::Path;
use std::time::Duration;

use crate::paths::{Paths, write_atomic};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(
        "The mihomo core is missing from this installation of Kairo. Reinstall Kairo to get it back."
    )]
    NotBundled,
    #[error("The mihomo core bundled with Kairo does not run: {0}")]
    Broken(String),
    #[error("Could not copy the core: {0}")]
    Io(String),
}

/// Makes `paths.core_bin()` the bundled core unless it already is (the same
/// version). Returns the version of the core in place.
pub async fn seed(paths: &Paths) -> Result<String, CoreError> {
    let target = paths.core_bin();
    let installed = binary_version(&target).await;
    let Some(bundled) = paths.bundled_core.clone() else {
        // A copy from an earlier run still works on its own.
        return installed.ok_or(CoreError::NotBundled);
    };
    let Some(version) = binary_version(&bundled).await else {
        return installed.ok_or_else(|| CoreError::Broken(bundled.display().to_string()));
    };
    if installed.as_deref() == Some(version.as_str()) {
        return Ok(version);
    }
    let bytes = tokio::fs::read(&bundled)
        .await
        .map_err(|e| CoreError::Io(e.to_string()))?;
    install_binary(paths, &bytes)?;
    log::info!("core {version} copied from {}", bundled.display());
    Ok(version)
}

/// Writes the core binary and makes it executable. Written fresh (not copied
/// with its metadata), so no quarantine or other attribute comes along.
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

/// The version a core binary reports (`mihomo -v`: "Mihomo Meta v1.19.33 linux amd64 ...").
pub async fn binary_version(core: &Path) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(core)
            .arg("-v")
            .kill_on_drop(true)
            .output(),
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
        .or_else(|| {
            text.split_whitespace()
                .find(|w| w.starts_with("alpha"))
                .map(str::to_owned)
        })
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
    fn versions() {
        assert_eq!(
            parse_version("Mihomo Meta v1.19.33 linux amd64 with go1.26.10 Wed Oct 21"),
            Some("v1.19.33".into())
        );
        assert!(is_newer("v1.19.33", "v1.19.18"));
        assert!(!is_newer("v1.19.33", "v1.19.33"));
        assert!(is_newer("v1.20.0", "v1.19.99"));
        assert!(is_newer("0.1.10", "0.1.9"));
    }

    #[cfg(unix)]
    fn fake_core(dir: &Path, version: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(format!("mihomo-{version}"));
        std::fs::write(
            &p,
            format!("#!/bin/sh\necho Mihomo Meta {version} linux amd64\n"),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn seeds_the_bundled_core_and_follows_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Paths::new(dir.path().join("home"));
        paths.bundled_core = None;
        assert!(matches!(seed(&paths).await, Err(CoreError::NotBundled)));

        // First run: copied in.
        paths.bundled_core = Some(fake_core(dir.path(), "v1.19.33"));
        assert_eq!(seed(&paths).await.unwrap(), "v1.19.33");
        assert_eq!(
            binary_version(&paths.core_bin()).await.as_deref(),
            Some("v1.19.33")
        );

        // Same version: the copy (and any grant on it) is left alone.
        let before = std::fs::metadata(paths.core_bin())
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        seed(&paths).await.unwrap();
        assert_eq!(
            std::fs::metadata(paths.core_bin())
                .unwrap()
                .modified()
                .unwrap(),
            before
        );

        // An app update brings another core: it replaces the copy.
        paths.bundled_core = Some(fake_core(dir.path(), "v1.19.34"));
        assert_eq!(seed(&paths).await.unwrap(), "v1.19.34");
        assert_eq!(
            binary_version(&paths.core_bin()).await.as_deref(),
            Some("v1.19.34")
        );

        // The bundle went missing: the copy keeps working.
        paths.bundled_core = Some(dir.path().join("gone"));
        assert_eq!(seed(&paths).await.unwrap(), "v1.19.34");
    }
}
