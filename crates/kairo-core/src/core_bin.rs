//! The mihomo core binary. Kairo ships one (see [`crate::paths::bundled_core`])
//! and runs a copy of it from its data directory, so that TUN's one-time grant
//! (file capabilities on Linux, setuid on macOS) is given to Kairo's own copy and
//! never to a file inside the app or a system package. The copy follows the
//! bundled core: when an app update brings the next pinned version, it is
//! replaced (see [`seed`]).

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

/// What [`seed`] found and did.
#[derive(Debug, Clone, PartialEq)]
pub struct Seeded {
    /// The version of the core in place.
    pub version: String,
    /// The copy in Kairo's folder was replaced (not a first copy): it has
    /// lost TUN's permission.
    pub replaced: Option<crate::view::CoreReplaced>,
}

/// The mihomo version Kairo ships (`MIHOMO_VERSION` in scripts/mihomo.env).
pub fn pinned_version() -> &'static str {
    include_str!("../../../scripts/mihomo.env")
        .lines()
        .find_map(|l| l.strip_prefix("MIHOMO_VERSION="))
        .map(str::trim)
        .expect("scripts/mihomo.env names MIHOMO_VERSION")
}

/// Makes `paths.core_bin()` the bundled core unless it already is.
///
/// Replacing the copy in Kairo's folder takes its TUN permission (a file
/// capability, the setuid bit) with it, so it happens only for an update:
///
/// - a copy with the bundled core's bytes is never touched (its permission,
///   inode and times stay), whatever the files' times say; nor one of the
///   same version;
/// - the bundled core replaces a copy only when it is the version this Kairo
///   ships ([`pinned_version`]) and not older than the copy;
/// - a replacement is said in [`Seeded::replaced`] (the state, `status`).
///
/// A first copy (the folder has none) is made whatever the bundled core is.
pub async fn seed(paths: &Paths) -> Result<Seeded, CoreError> {
    let target = paths.core_bin();
    let Some(bundled) = paths.bundled_core.clone() else {
        // A copy from an earlier run still works on its own.
        let installed = binary_version(&target).await;
        return installed.map(|version| Seeded { version, replaced: None }).ok_or(CoreError::NotBundled);
    };
    let Some(version) = binary_version(&bundled).await else {
        let installed = binary_version(&target).await;
        return installed
            .map(|version| Seeded { version, replaced: None })
            .ok_or_else(|| CoreError::Broken(bundled.display().to_string()));
    };
    if same_content(&target, &bundled) {
        return Ok(Seeded { version, replaced: None });
    }
    let installed = binary_version(&target).await;
    if installed.as_deref() == Some(version.as_str()) {
        return Ok(Seeded { version, replaced: None });
    }
    let existed = target.exists();
    if existed {
        let pinned = pinned_version();
        let not_this = if version != pinned {
            Some(format!("it is {version}, not the core this Kairo ships ({pinned})"))
        } else {
            installed
                .as_deref()
                .filter(|i| is_newer(i, &version))
                .map(|i| format!("it is {version}, older than the folder's {i}"))
        };
        if let Some(why) = not_this {
            log::warn!("core: {} left as it is, {} not copied: {why}", target.display(), bundled.display());
            return installed
                .map(|version| Seeded { version, replaced: None })
                .ok_or_else(|| CoreError::Io(format!("The core at {} does not run, and {} is not one to replace it with: {why}", target.display(), bundled.display())));
        }
    } else if version != pinned_version() {
        log::warn!("core: {} is {version}, not the one this Kairo ships ({}); copied as the first core", bundled.display(), pinned_version());
    }
    let bytes = tokio::fs::read(&bundled)
        .await
        .map_err(|e| CoreError::Io(e.to_string()))?;
    install_binary(paths, &bytes)?;
    let replaced = existed.then(|| crate::view::CoreReplaced { from: installed.clone(), to: version.clone() });
    match &replaced {
        Some(r) => log::warn!(
            "core {} replaced by {version} from {}: TUN needs its permission again",
            r.from.as_deref().unwrap_or("(did not run)"),
            bundled.display()
        ),
        None => log::info!("core {version} copied from {}", bundled.display()),
    }
    Ok(Seeded { version, replaced })
}

/// Whether `a` and `b` are both there with the same bytes.
fn same_content(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    if ma.len() != mb.len() {
        return false;
    }
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
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
    let mut command = tokio::process::Command::new(core);
    command.arg("-v").kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let out = tokio::time::timeout(Duration::from_secs(10), command.output())
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

        let pinned = pinned_version();
        assert!(pinned.starts_with('v'), "{pinned}");

        // First run: copied in (nothing replaced), whatever its version.
        paths.bundled_core = Some(fake_core(dir.path(), "v1.0.0"));
        assert_eq!(seed(&paths).await.unwrap(), Seeded { version: "v1.0.0".into(), replaced: None });
        assert_eq!(
            binary_version(&paths.core_bin()).await.as_deref(),
            Some("v1.0.0")
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

        // An app update brings the pinned core: it replaces the copy, and says so.
        paths.bundled_core = Some(fake_core(dir.path(), pinned));
        assert_eq!(
            seed(&paths).await.unwrap(),
            Seeded {
                version: pinned.into(),
                replaced: Some(crate::view::CoreReplaced { from: Some("v1.0.0".into()), to: pinned.into() })
            }
        );
        assert_eq!(binary_version(&paths.core_bin()).await.as_deref(), Some(pinned));

        // A core that is not the pinned one (another mihomo) never replaces it.
        let kept = std::fs::metadata(paths.core_bin()).unwrap().modified().unwrap();
        for other in ["v1.0.1", "v99.0.0"] {
            paths.bundled_core = Some(fake_core(dir.path(), other));
            assert_eq!(seed(&paths).await.unwrap(), Seeded { version: pinned.into(), replaced: None }, "{other}");
            assert_eq!(std::fs::metadata(paths.core_bin()).unwrap().modified().unwrap(), kept, "{other}");
        }

        // Nor does the pinned core replace a newer one in the folder.
        install_binary(&paths, &std::fs::read(fake_core(dir.path(), "v99.0.0")).unwrap()).unwrap();
        paths.bundled_core = Some(fake_core(dir.path(), pinned));
        assert_eq!(seed(&paths).await.unwrap(), Seeded { version: "v99.0.0".into(), replaced: None });

        // The bundle went missing: the copy keeps working.
        paths.bundled_core = Some(dir.path().join("gone"));
        assert_eq!(seed(&paths).await.unwrap().version, "v99.0.0");
    }
}
