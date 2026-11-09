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
    pub fn core_dir(&self) -> PathBuf {
        self.home.join("core")
    }
    pub fn core_bin(&self) -> PathBuf {
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
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    bundled_core_for(&exe)
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
/// crash never leaves a half-written file behind.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}
