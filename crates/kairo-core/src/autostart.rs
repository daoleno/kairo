//! Launch at login: an XDG autostart entry on Linux, a LaunchAgent on macOS, a
//! `Run` value of the current user on Windows (no administrator needed).

use std::path::PathBuf;

const ID: &str = "dev.kairo.app";

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// The command that starts this app (inside an .app bundle, its executable).
fn executable() -> Result<String, String> {
    crate::paths::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| "cannot find this program".to_string())
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

/// The `Run` values on Windows: Kairo at login, and the network check.
#[cfg(windows)]
const RUN_NAME: &str = "Kairo";
#[cfg(windows)]
const RUN_RESTORE_NAME: &str = "KairoNetworkCheck";

/// The command line of a `Run` value: the quoted executable and its flag.
pub fn run_value(exe: &str, flag: Option<&str>) -> String {
    match flag {
        Some(f) => format!("\"{exe}\" {f}"),
        None => format!("\"{exe}\""),
    }
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
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{ID}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{exe}</string>{extra}\n  </array>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>ProcessType</key>\n  <string>Interactive</string>\n  <key>AssociatedBundleIdentifiers</key>\n  <array>\n    <string>dev.kairo.app</string>\n  </array>\n</dict>\n</plist>\n"
        )
    } else {
        format!(
            "[Desktop Entry]\nType=Application\nName=Kairo\nComment=mihomo client\nExec=\"{exe}\"{flag}\nIcon={ID}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        )
    }
}

pub fn is_enabled() -> bool {
    #[cfg(windows)]
    {
        crate::winapi::reg_get_string(crate::winapi::RUN_KEY, RUN_NAME).is_some()
    }
    #[cfg(not(windows))]
    {
        entry_path().exists()
    }
}

/// Turns launch at login on or off.
pub fn set(on: bool, minimized: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use crate::winapi::{RUN_KEY, reg_delete, reg_set_string};
        return if on {
            let exe = executable()?;
            reg_set_string(
                RUN_KEY,
                RUN_NAME,
                &run_value(&exe, minimized.then_some("--minimized")),
            )
        } else {
            reg_delete(RUN_KEY, RUN_NAME)
        };
    }
    #[allow(unreachable_code)]
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
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{RESTORE_ID}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{exe}</string>\n    <string>--restore-network</string>\n  </array>{env}\n  <key>RunAtLoad</key>\n  <true/>\n  <key>ProcessType</key>\n  <string>Background</string>\n  <key>AssociatedBundleIdentifiers</key>\n  <array>\n    <string>dev.kairo.app</string>\n  </array>\n</dict>\n</plist>\n"
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
    #[cfg(windows)]
    {
        use crate::winapi::{RUN_KEY, reg_get_string, reg_set_string};
        let exe = executable()?;
        let check = run_value(&exe, Some("--restore-network"));
        if reg_get_string(RUN_KEY, RUN_RESTORE_NAME).as_deref() != Some(check.as_str()) {
            reg_set_string(RUN_KEY, RUN_RESTORE_NAME, &check)?;
        }
        // Launch at login follows the executable when it moved (an update, a new folder).
        if let Some(current) = reg_get_string(RUN_KEY, RUN_NAME) {
            let flag = current.contains("--minimized").then_some("--minimized");
            let wanted = run_value(&exe, flag);
            if current != wanted {
                reg_set_string(RUN_KEY, RUN_NAME, &wanted)?;
            }
        }
        return Ok(());
    }
    #[allow(unreachable_code)]
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
    fn run_value_quotes_the_path() {
        assert_eq!(
            super::run_value("C:\\Users\\a b\\kairo.exe", Some("--minimized")),
            "\"C:\\Users\\a b\\kairo.exe\" --minimized"
        );
        assert_eq!(super::run_value("k.exe", None), "\"k.exe\"");
    }

    #[test]
    fn entry_has_the_command() {
        let e = super::entry("/opt/kairo/kairo", true);
        assert!(e.contains("/opt/kairo/kairo"));
        assert!(e.contains("--minimized"));
    }
}
