//! Launch at login: an XDG autostart entry on Linux, a LaunchAgent on macOS.

use std::path::PathBuf;

const ID: &str = "dev.kairo.app";

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// The command that starts this app (inside an .app bundle, its executable).
fn executable() -> Result<String, String> {
    std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn entry_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"))
        .join("autostart")
        .join(format!("{ID}.desktop"))
}

#[cfg(target_os = "macos")]
fn entry_path() -> PathBuf {
    home()
        .join("Library/LaunchAgents")
        .join(format!("{ID}.plist"))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn entry_path() -> PathBuf {
    PathBuf::new()
}

/// The autostart entry for `exe`.
pub fn entry(exe: &str, minimized: bool) -> String {
    let flag = if minimized { " --minimized" } else { "" };
    if cfg!(target_os = "macos") {
        let extra = if minimized {
            "\n    <string>--minimized</string>"
        } else {
            ""
        };
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{ID}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{exe}</string>{extra}\n  </array>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>ProcessType</key>\n  <string>Interactive</string>\n</dict>\n</plist>\n"
        )
    } else {
        format!(
            "[Desktop Entry]\nType=Application\nName=Kairo\nComment=mihomo client\nExec=\"{exe}\"{flag}\nIcon={ID}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        )
    }
}

pub fn is_enabled() -> bool {
    entry_path().exists()
}

/// Turns launch at login on or off.
pub fn set(on: bool, minimized: bool) -> Result<(), String> {
    if cfg!(not(any(target_os = "linux", target_os = "macos"))) {
        return Err("not supported on this platform".into());
    }
    let path = entry_path();
    if on {
        let exe = executable()?;
        crate::paths::write_atomic(&path, entry(&exe, minimized).as_bytes())
            .map_err(|e| e.to_string())
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

// --- The network check at login ------------------------------------------------

/// Its own entry, apart from launch at login (turning that off never removes it).
const RESTORE_ID: &str = "dev.kairo.restore-network";

fn restore_entry_path() -> PathBuf {
    let launch = entry_path();
    match launch.parent() {
        Some(dir) if !launch.as_os_str().is_empty() => dir.join(format!(
            "{RESTORE_ID}.{}",
            if cfg!(target_os = "macos") {
                "plist"
            } else {
                "desktop"
            }
        )),
        _ => PathBuf::new(),
    }
}

/// At login, `kairo --restore-network`: when Kairo and its core were both
/// killed (a forced logout, a power cut) and the computer restarts, the system
/// proxy would still point at a port nothing answers; this puts it back, even
/// if Kairo itself never opens again. With nothing to restore it does nothing.
pub fn restore_entry(exe: &str, kairo_home: Option<&str>) -> String {
    if cfg!(target_os = "macos") {
        let env = kairo_home
            .map(|h| format!("\n  <key>EnvironmentVariables</key>\n  <dict>\n    <key>KAIRO_HOME</key>\n    <string>{h}</string>\n  </dict>"))
            .unwrap_or_default();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{RESTORE_ID}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{exe}</string>\n    <string>--restore-network</string>\n  </array>{env}\n  <key>RunAtLoad</key>\n  <true/>\n  <key>ProcessType</key>\n  <string>Background</string>\n</dict>\n</plist>\n"
        )
    } else {
        let env = kairo_home
            .map(|h| format!("env KAIRO_HOME=\"{h}\" "))
            .unwrap_or_default();
        format!(
            "[Desktop Entry]\nType=Application\nName=Kairo network check\nComment=Puts the system proxy back if Kairo could not\nExec={env}\"{exe}\" --restore-network\nNoDisplay=true\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        )
    }
}

/// Puts the login check in place (once; rewritten when Kairo moved).
pub fn install_restore() -> Result<(), String> {
    if cfg!(not(any(target_os = "linux", target_os = "macos"))) {
        return Ok(());
    }
    let path = restore_entry_path();
    let home = std::env::var("KAIRO_HOME").ok();
    let text = restore_entry(&executable()?, home.as_deref());
    if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    crate::paths::write_atomic(&path, text.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn restore_entry_runs_the_check() {
        let e = super::restore_entry("/opt/kairo/kairo", Some("/tmp/k"));
        assert!(e.contains("/opt/kairo/kairo"));
        assert!(e.contains("--restore-network"));
        assert!(e.contains("/tmp/k"));
    }

    #[test]
    fn entry_has_the_command() {
        let e = super::entry("/opt/kairo/kairo", true);
        assert!(e.contains("/opt/kairo/kairo"));
        assert!(e.contains("--minimized"));
    }
}
