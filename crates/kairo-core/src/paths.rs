//! Where Kairo keeps its files.
//!
//! ```text
//! <home>/
//!   settings.json        app settings
//!   profiles.json        the profile list (metadata)
//!   profiles/<id>.yaml   each profile's config, as downloaded or imported
//!   overrides/<id>.yaml  the user's merge on top of a profile
//!   core/mihomo          the core Kairo runs: a copy of the one bundled with the app
//!                        (a copy so that TUN's one-time grant never touches the app itself)
//!   update/              a verified Kairo update, waiting for a restart
//!   runtime/             the core's working directory (-d): config.yaml, geo files, cache.db
//!   state/sysproxy.json  the system proxy as it was before Kairo changed it (crash recovery)
//!   logs/core.log        the core's recent output
//! ```
//!
//! `<home>` is `KAIRO_HOME` when set (tests, isolated runs), else the platform's
//! application data directory: `$XDG_DATA_HOME/kairo` (`~/.local/share/kairo`) on
//! Linux, `~/Library/Application Support/Kairo` on macOS, `%APPDATA%\Kairo` on Windows.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    /// The mihomo core shipped with this copy of Kairo (see [`bundled_core`]).
    pub bundled_core: Option<PathBuf>,
}

impl Paths {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            bundled_core: bundled_core(),
        }
    }

    /// The default location (see the module docs).
    pub fn default_home() -> PathBuf {
        if let Some(home) = std::env::var_os("KAIRO_HOME") {
            return PathBuf::from(home);
        }
        // Android: the app's own files directory (the Java side tells us).
        #[cfg(target_os = "android")]
        if let Some(files) = crate::android::files_dir() {
            return files;
        }
        let user_home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        if cfg!(target_os = "macos") {
            user_home.join("Library/Application Support/Kairo")
        } else if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or(user_home)
                .join("Kairo")
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| user_home.join(".local/share"))
                .join("kairo")
        }
    }

    pub fn settings(&self) -> PathBuf {
        self.home.join("settings.json")
    }
    pub fn profiles_index(&self) -> PathBuf {
        self.home.join("profiles.json")
    }
    pub fn profiles_dir(&self) -> PathBuf {
        self.home.join("profiles")
    }
    pub fn profile(&self, id: &str) -> PathBuf {
        self.profiles_dir().join(format!("{id}.yaml"))
    }
    pub fn override_file(&self, id: &str) -> PathBuf {
        self.home.join("overrides").join(format!("{id}.yaml"))
    }
    /// The pictures of apps and sites (`icons`).
    pub fn icons_dir(&self) -> PathBuf {
        self.home.join("icons")
    }
    pub fn core_dir(&self) -> PathBuf {
        self.home.join("core")
    }
    pub fn core_bin(&self) -> PathBuf {
        // Android: the core runs where the APK installer put it (an app may
        // not execute a file from its data directory).
        #[cfg(target_os = "android")]
        if let Some(core) = crate::android::core() {
            return core;
        }
        self.core_dir().join(if cfg!(windows) {
            "mihomo.exe"
        } else {
            "mihomo"
        })
    }
    pub fn update_dir(&self) -> PathBuf {
        self.home.join("update")
    }
    pub fn runtime_dir(&self) -> PathBuf {
        self.home.join("runtime")
    }
    pub fn runtime_config(&self) -> PathBuf {
        self.runtime_dir().join("config.yaml")
    }
    pub fn candidate_config(&self) -> PathBuf {
        self.runtime_dir().join("candidate.yaml")
    }
    pub fn state_dir(&self) -> PathBuf {
        self.home.join("state")
    }
    pub fn sysproxy_state(&self) -> PathBuf {
        self.state_dir().join("sysproxy.json")
    }
    pub fn core_pid(&self) -> PathBuf {
        self.state_dir().join("core.pid")
    }
    /// What the statistics pages show, kept 30 days; nothing else reads it.
    pub fn stats_file(&self) -> PathBuf {
        self.home.join("stats.json")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.home.join("logs")
    }

    /// Creates the directories Kairo writes to.
    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            self.home.clone(),
            self.profiles_dir(),
            self.home.join("overrides"),
            self.core_dir(),
            self.runtime_dir(),
            self.state_dir(),
            self.logs_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

/// The core that ships with the running Kairo, found next to its executable:
///
/// - macOS: `Kairo.app/Contents/MacOS/mihomo`;
/// - Linux: `<prefix>/lib/kairo/mihomo` for `<prefix>/bin/kairo` (`/usr` for the
///   .deb, `~/.local` for the tarball's install.sh);
/// - a development build: `target/mihomo/mihomo` (`scripts/fetch-mihomo.sh`).
pub fn bundled_core() -> Option<PathBuf> {
    #[cfg(target_os = "android")]
    return crate::android::core();
    #[cfg(not(target_os = "android"))]
    bundled_core_for(&current_exe()?)
}

/// This executable's real path. On Windows `canonicalize` returns the
/// `\\?\C:\...` form, which is a poor thing to write into a Run entry or
/// compare with a process's image path, so the plain drive path is used
/// whenever there is one.
pub fn current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(plain(exe.canonicalize().unwrap_or(exe)))
}

/// `\\?\C:\dir` as `C:\dir`; every other path as it is.
pub fn plain(p: PathBuf) -> PathBuf {
    let stripped = p
        .to_string_lossy()
        .strip_prefix(r"\\?\")
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
        .map(PathBuf::from);
    stripped.unwrap_or(p)
}

pub fn bundled_core_for(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let name = if cfg!(windows) {
        "mihomo.exe"
    } else {
        "mihomo"
    };
    [
        dir.join(name),
        dir.join("../lib/kairo").join(name),
        dir.join("../mihomo").join(name),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Writes `bytes` to `path` through a temporary file in the same directory, so a
/// crash never leaves a half-written file behind. On Unix only the owner may
/// read it: Kairo's files hold subscription links, node passwords, the
/// controller's secret and what was visited (a program that must run is
/// made executable after, see `core_bin::install_binary`).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Makes what Kairo wrote before its files were private (an older version's)
/// its owner's alone: the folder, and every file in it that may carry a
/// secret. Executable bits stay (the core must run).
#[cfg(unix)]
pub fn make_private(paths: &Paths) {
    use std::os::unix::fs::PermissionsExt;
    let private = |p: &Path| {
        if let Ok(m) = std::fs::symlink_metadata(p)
            && !m.file_type().is_symlink()
            && m.permissions().mode() & 0o077 != 0
        {
            let mode = m.permissions().mode() & 0o700;
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
        }
    };
    private(&paths.home);
    let files_of = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect())
            .unwrap_or_default()
    };
    for f in files_of(&paths.home) {
        private(&f);
    }
    for dir in ["profiles", "overrides", "runtime", "state", "logs", "update", "web"] {
        let dir = paths.home.join(dir);
        private(&dir);
        for f in files_of(&dir) {
            private(&f);
        }
    }
}

#[cfg(test)]
mod plain_tests {
    use super::plain;
    use std::path::PathBuf;

    #[test]
    fn the_verbatim_prefix_goes_only_from_drive_paths() {
        assert_eq!(
            plain(PathBuf::from(r"\\?\C:\Users\a\kairo.exe")),
            PathBuf::from(r"C:\Users\a\kairo.exe")
        );
        assert_eq!(
            plain(PathBuf::from(r"\\?\UNC\host\share\k.exe")),
            PathBuf::from(r"\\?\UNC\host\share\k.exe")
        );
        assert_eq!(
            plain(PathBuf::from("/usr/bin/kairo")),
            PathBuf::from("/usr/bin/kairo")
        );
    }
}

#[cfg(all(test, unix))]
mod private_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn what_kairo_writes_is_its_owners_alone() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths { home: dir.path().join("kairo"), bundled_core: None };
        write_atomic(&paths.settings(), b"{\"secret\":\"s\"}").unwrap();
        assert_eq!(mode(&paths.settings()), 0o600);
        // An older Kairo's files, readable by everyone, and a core that must run.
        let old = paths.profile("a");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "proxies: []").unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::set_permissions(&paths.home, std::fs::Permissions::from_mode(0o755)).unwrap();
        let core = paths.core_dir().join("mihomo");
        std::fs::create_dir_all(paths.core_dir()).unwrap();
        std::fs::write(&core, "#!/bin/sh").unwrap();
        std::fs::set_permissions(&core, std::fs::Permissions::from_mode(0o755)).unwrap();
        make_private(&paths);
        assert_eq!(mode(&old), 0o600);
        assert_eq!(mode(&paths.home), 0o700);
        assert_eq!(mode(&core), 0o755, "the core is left as it is");
    }
}
