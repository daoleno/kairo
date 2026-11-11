//! Runs the core as a child process: its output kept for diagnosis, and a pid
//! file so a core left behind by a crash is found and stopped.

use std::path::{Path, PathBuf};
use std::process::Stdio;
#[cfg_attr(windows, allow(unused_imports))]
use std::time::Duration;

use crate::paths::Paths;

const TAIL: usize = 300;

pub struct CoreProcess {
    /// The core as Kairo's own child; none when it runs as SYSTEM (Windows TUN),
    /// where a user can neither open its process nor wait on it: it is started and
    /// ended through its task or service, and seen through its named event.
    child: Option<tokio::process::Child>,
    #[cfg(windows)]
    elevated: Option<(crate::elevated::Route, String)>,
    pub pid: u32,
    log: PathBuf,
}

/// What the pid file holds when the core was started by the task or service.
#[cfg(windows)]
const ELEVATED_MARK: &str = "elevated";

/// How the core ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    /// The last lines it printed.
    pub tail: Vec<String>,
}

impl Exit {
    /// Why it stopped, in words for the user: the core's last error line if any.
    pub fn reason(&self) -> String {
        let last_error = self
            .tail
            .iter()
            .rev()
            .find(|l| {
                let l = l.to_ascii_lowercase();
                l.contains("level=fatal") || l.contains("level=error") || l.contains("panic")
            })
            .map(|l| crate::config::explain(l));
        match (last_error, self.code, self.signal) {
            (Some(e), _, _) => e,
            (None, Some(c), _) => format!("The core exited with code {c}"),
            (None, None, Some(s)) => format!("The core was stopped by signal {s}"),
            _ => "The core stopped".into(),
        }
    }
}

impl CoreProcess {
    /// Starts `core` on the runtime config. Fails when it cannot be executed.
    pub async fn spawn(core: &Path, paths: &Paths) -> std::io::Result<CoreProcess> {
        stop_stale(paths).await;
        let mut command = tokio::process::Command::new(core);
        command
            .arg("-d")
            .arg(paths.runtime_dir())
            .arg("-f")
            .arg(paths.runtime_config())
            .current_dir(paths.runtime_dir())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        // No console window (Kairo is a windowed program; the core is a console one).
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        // The core writes its output to the log file itself, not into a pipe
        // Kairo reads: a pipe would break when Kairo dies, and the core's next
        // line would end it (SIGPIPE).
        let log = paths.logs_dir().join("core.log");
        let _ = std::fs::create_dir_all(paths.logs_dir());
        match std::fs::File::create(&log).and_then(|f| Ok((f.try_clone()?, f))) {
            Ok((out, err)) => command.stdout(out).stderr(err),
            Err(e) => {
                log::warn!("core log {}: {e}", log.display());
                command.stdout(Stdio::null()).stderr(Stdio::null())
            }
        };
        // No PR_SET_PDEATHSIG either: when Kairo dies, its guardian puts the
        // system proxy back and then stops the core. If the guardian is gone too
        // (both killed at once), the core going on keeps the proxy the system
        // points at alive, so the computer stays online; the next Kairo stops it.
        let child = command.spawn()?;
        let pid = child.id().unwrap_or(0);
        let _ = std::fs::write(paths.core_pid(), pid.to_string());
        Ok(CoreProcess {
            child: Some(child),
            #[cfg(windows)]
            elevated: None,
            pid,
            log,
        })
    }

    /// Starts the core through its scheduled task or service (Windows TUN): no
    /// prompt, the one-time grant gave this user the right. Waits until the host is
    /// up, so `wait` can tell "ended" from "not started yet".
    #[cfg(windows)]
    pub async fn spawn_elevated(paths: &Paths) -> std::io::Result<CoreProcess> {
        stop_stale(paths).await;
        let (route, name) = crate::elevated::current().ok_or_else(|| {
            std::io::Error::other("TUN needs permission first (Settings, Every app)")
        })?;
        let log = paths.logs_dir().join("core.log");
        let _ = std::fs::create_dir_all(paths.logs_dir());
        // The host starts a fresh log; do not read the last one meanwhile.
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::write(paths.core_pid(), ELEVATED_MARK);
        let n = name.clone();
        let started = tokio::task::spawn_blocking(move || crate::elevated::start(route, &n))
            .await
            .map_err(std::io::Error::other)?;
        if let Err(e) = started {
            let _ = std::fs::remove_file(paths.core_pid());
            return Err(std::io::Error::other(e));
        }
        for _ in 0..50 {
            if crate::elevated::running(&name) {
                return Ok(CoreProcess {
                    child: None,
                    elevated: Some((route, name)),
                    pid: 0,
                    log,
                });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = std::fs::remove_file(paths.core_pid());
        let tail = tail_of(&log, 3).join(" ");
        Err(std::io::Error::other(format!(
            "The core did not start through its task{}",
            if tail.is_empty() { String::new() } else { format!(": {tail}") }
        )))
    }

    /// The last lines it printed.
    pub fn tail(&self) -> Vec<String> {
        tail_of(&self.log, TAIL)
    }

    /// Whether it is still running.
    pub fn running(&mut self) -> bool {
        #[cfg(windows)]
        if let Some((_, name)) = &self.elevated {
            return crate::elevated::running(name);
        }
        matches!(self.child.as_mut().map(|c| c.try_wait()), Some(Ok(None)))
    }

    /// Waits for it to exit (cancel-safe).
    pub async fn wait(&mut self) -> Exit {
        #[cfg(windows)]
        if let Some((_, name)) = &self.elevated {
            // Gone twice in a row (a moment between two instances is not an end).
            let mut gone = 0;
            while gone < 2 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                gone = if crate::elevated::running(name) { 0 } else { gone + 1 };
            }
            return Exit {
                code: None,
                signal: None,
                tail: self.tail(),
            };
        }
        let Some(child) = self.child.as_mut() else {
            std::future::pending::<()>().await;
            unreachable!()
        };
        let status = child.wait().await;
        let (code, signal) = match status {
            Ok(s) => (s.code(), signal_of(&s)),
            Err(_) => (None, None),
        };
        Exit {
            code,
            signal,
            tail: self.tail(),
        }
    }

    /// Asks it to stop, then kills it after a grace period.
    pub async fn stop(&mut self, paths: &Paths) {
        #[cfg(windows)]
        if let Some((route, name)) = self.elevated.clone() {
            stop_elevated(route, &name).await;
            let _ = std::fs::remove_file(paths.core_pid());
            return;
        }
        let Some(child) = self.child.as_mut() else {
            let _ = std::fs::remove_file(paths.core_pid());
            return;
        };
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
            if tokio::time::timeout(Duration::from_secs(3), child.wait())
                .await
                .is_ok()
            {
                let _ = std::fs::remove_file(paths.core_pid());
                return;
            }
        }
        let _ = child.kill().await;
        let _ = std::fs::remove_file(paths.core_pid());
    }
}

/// Ends the task or service and waits (a few seconds) until the core is gone.
#[cfg(windows)]
async fn stop_elevated(route: crate::elevated::Route, name: &str) {
    let n = name.to_owned();
    let _ = tokio::task::spawn_blocking(move || crate::elevated::stop(route, &n)).await;
    for _ in 0..50 {
        if !crate::elevated::running(name) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(unix)]
fn signal_of(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn signal_of(_: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// The last `n` lines of a file.
fn tail_of(path: &Path, n: usize) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

/// Stops a core a previous Kairo left running (after a crash), identified by
/// the pid file and its command line naming this runtime directory.
pub async fn stop_stale(paths: &Paths) {
    let Ok(text) = std::fs::read_to_string(paths.core_pid()) else {
        return;
    };
    let _ = std::fs::remove_file(paths.core_pid());
    #[cfg(windows)]
    if text.trim() == ELEVATED_MARK {
        // Started by the task or service: end it the same way.
        if let Some((route, name)) = crate::elevated::current()
            && crate::elevated::running(&name)
        {
            log::warn!("stopping a core left running by a previous session (its task)");
            stop_elevated(route, &name).await;
        }
        return;
    }
    let Ok(pid) = text.trim().parse::<i32>() else {
        return;
    };
    if pid <= 1 || !is_our_core(pid, paths) {
        return;
    }
    log::warn!("stopping a core left running by a previous session (pid {pid})");
    stop_pid(pid).await;
}

/// SIGTERM, three seconds, SIGKILL; returns once the process is gone (or
/// after another second), so its ports are free for the next core.
pub async fn stop_pid(pid: i32) {
    #[cfg(windows)]
    {
        let pid = pid as u32;
        if crate::winapi::terminate(pid) {
            let _ =
                tokio::task::spawn_blocking(move || crate::winapi::wait_for_exit(pid, 4000)).await;
        }
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(pid, libc::SIGTERM);
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if libc::kill(pid, 0) != 0 {
                return;
            }
        }
        libc::kill(pid, libc::SIGKILL);
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if libc::kill(pid, 0) != 0 {
                return;
            }
        }
    }
}

/// Whether this Kairo's core is running (starting, or left by a Kairo that is gone).
pub fn core_alive(paths: &Paths) -> bool {
    #[cfg(windows)]
    if std::fs::read_to_string(paths.core_pid()).is_ok_and(|t| t.trim() == ELEVATED_MARK) {
        return crate::elevated::current().is_some_and(|(_, name)| crate::elevated::running(&name));
    }
    let Some(pid) = std::fs::read_to_string(paths.core_pid())
        .ok()
        .and_then(|t| t.trim().parse::<i32>().ok())
    else {
        return false;
    };
    #[cfg(unix)]
    let alive = pid > 1 && unsafe { libc::kill(pid, 0) } == 0;
    #[cfg(windows)]
    let alive = pid > 1 && crate::winapi::pid_alive(pid as u32);
    #[cfg(not(any(unix, windows)))]
    let alive = pid > 1;
    alive && is_our_core(pid, paths)
}

fn is_our_core(pid: i32, paths: &Paths) -> bool {
    let runtime = paths.runtime_dir().to_string_lossy().into_owned();
    #[cfg(target_os = "linux")]
    {
        std::fs::read(format!("/proc/{pid}/cmdline"))
            .map(|c| {
                String::from_utf8_lossy(&c)
                    .replace('\0', " ")
                    .contains(&runtime)
            })
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        // The core Kairo runs is the copy in its own `core` directory.
        let _ = runtime;
        crate::winapi::image_path(pid as u32).is_some_and(|p| same_path(&p, &paths.core_bin()))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&runtime))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_reason_prefers_the_core_error() {
        let e = Exit {
            code: Some(1),
            signal: None,
            tail: vec![
                "time=\"x\" level=info msg=\"Start\"".into(),
                "time=\"x\" level=fatal msg=\"Parse config error: proxy 'x' not found\"".into(),
            ],
        };
        assert_eq!(e.reason(), "Parse config error: proxy 'x' not found");
        let e = Exit {
            code: Some(2),
            signal: None,
            tail: vec![],
        };
        assert_eq!(e.reason(), "The core exited with code 2");
    }
}

/// Whether two Windows paths name the same file (case and slashes aside).
pub fn same_path(a: &str, b: &Path) -> bool {
    let norm = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(a) == norm(&b.to_string_lossy())
}
