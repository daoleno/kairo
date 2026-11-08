//! What TUN mode needs: the core must be allowed to create a network device and
//! change routes.
//!
//! - Linux: the core binary gets the file capabilities `cap_net_admin` and
//!   `cap_net_bind_service` once, through `pkexec setcap` (polkit asks for the
//!   password). Kairo itself never runs privileged; only the core can do this one
//!   thing. Replacing the core (an update) drops the capabilities, and Kairo asks again.
//!   A core run as root (or inside a user+network namespace, where Kairo is uid 0)
//!   needs nothing.
//! - macOS: the core is made setuid root once, through the system's administrator
//!   prompt (`osascript ... with administrator privileges`).

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    /// The core can run TUN.
    Granted,
    /// It needs the one-time grant.
    Needed,
    /// TUN is not available on this platform.
    Unsupported,
}

pub fn status(core: &Path) -> Privilege {
    if !core.exists() {
        return Privilege::Needed;
    }
    #[cfg(unix)]
    if unsafe { libc::geteuid() } == 0 {
        return Privilege::Granted;
    }
    #[cfg(target_os = "linux")]
    {
        if has_net_admin(core) { Privilege::Granted } else { Privilege::Needed }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt;
        match std::fs::metadata(core) {
            Ok(m) if m.uid() == 0 && m.mode() & 0o4000 != 0 => Privilege::Granted,
            _ => Privilege::Needed,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = core;
        Privilege::Unsupported
    }
}

/// Whether the file capabilities of `core` include CAP_NET_ADMIN.
#[cfg(target_os = "linux")]
pub fn has_net_admin(core: &Path) -> bool {
    // `security.capability` xattr: vfs_cap_data; permitted bits start at byte 4.
    // CAP_NET_ADMIN is bit 12.
    let mut buf = [0u8; 64];
    let path = std::ffi::CString::new(core.as_os_str().as_encoded_bytes()).expect("path");
    let name = c"security.capability";
    let n = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    if n < 8 {
        return false;
    }
    let permitted = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    permitted & (1 << 12) != 0
}

/// Asks the user (polkit / the macOS admin prompt) to let the core run TUN.
pub fn grant(core: &Path) -> Result<(), String> {
    let path = core.to_string_lossy().into_owned();
    #[cfg(target_os = "linux")]
    {
        let setcap = ["/usr/bin/setcap", "/usr/sbin/setcap", "/sbin/setcap"]
            .into_iter()
            .find(|p| Path::new(p).exists())
            .ok_or("setcap is not installed (libcap)")?;
        let out = Command::new("pkexec")
            .args([setcap, "cap_net_admin,cap_net_bind_service=+ep", &path])
            .output()
            .map_err(|e| format!("pkexec: {e}"))?;
        return match out.status.code() {
            Some(0) if has_net_admin(core) => Ok(()),
            Some(0) => Err("The permission did not stick (is the core on a filesystem without capabilities?)".into()),
            Some(126) => Err("Cancelled".into()),
            Some(127) => Err("No polkit agent could ask for the password".into()),
            _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()),
        };
    }
    #[cfg(target_os = "macos")]
    {
        let quoted = path.replace('\'', "'\\''");
        let script = format!(
            "do shell script \"chown root:admin '{quoted}' && chmod u+s,a+x '{quoted}'\" with administrator privileges with prompt \"Kairo needs permission to start TUN mode.\""
        );
        let out = Command::new("/usr/bin/osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| format!("osascript: {e}"))?;
        return if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(if err.contains("-128") { "Cancelled".into() } else { err.trim().to_owned() })
        };
    }
    #[allow(unreachable_code)]
    {
        let _ = (path, Command::new("true"));
        Err("TUN is not supported on this platform".into())
    }
}
