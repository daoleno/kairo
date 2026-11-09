//! Runs the core as a child process: its output kept for diagnosis, a pid file
//! so a core left behind by a crash is found and stopped, and (on Linux) the
//! kernel told to stop the core if Kairo dies.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::paths::Paths;

const TAIL: usize = 300;

pub struct CoreProcess {
    child: tokio::process::Child,
    pub pid: u32,
    tail: Arc<Mutex<VecDeque<String>>>,
}

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
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(target_os = "linux")]
        unsafe {
            // The core goes when Kairo goes, even when Kairo is killed: a core
            // nobody controls would keep the system proxy pointing at it.
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let pid = child.id().unwrap_or(0);
        let _ = std::fs::write(paths.core_pid(), pid.to_string());
        let tail = Arc::new(Mutex::new(VecDeque::with_capacity(TAIL)));
        let log_path = paths.logs_dir().join("core.log");
        let log = Arc::new(Mutex::new(std::fs::File::create(&log_path).ok()));
        if let Some(out) = child.stdout.take() {
            tokio::spawn(pump(BufReader::new(out), tail.clone(), log.clone()));
        }
        if let Some(err) = child.stderr.take() {
            tokio::spawn(pump(BufReader::new(err), tail.clone(), log.clone()));
        }
        Ok(CoreProcess { child, pid, tail })
    }

    pub fn tail(&self) -> Vec<String> {
        self.tail
            .lock()
            .map(|t| t.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Whether it is still running.
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Waits for it to exit (cancel-safe).
    pub async fn wait(&mut self) -> Exit {
        let status = self.child.wait().await;
        // Let the output pumps take the last lines.
        tokio::time::sleep(Duration::from_millis(50)).await;
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
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
            if tokio::time::timeout(Duration::from_secs(3), self.child.wait())
                .await
                .is_ok()
            {
                let _ = std::fs::remove_file(paths.core_pid());
                return;
            }
        }
        let _ = self.child.kill().await;
        let _ = std::fs::remove_file(paths.core_pid());
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

async fn pump<R: tokio::io::AsyncRead + Unpin>(
    reader: BufReader<R>,
    tail: Arc<Mutex<VecDeque<String>>>,
    log: Arc<Mutex<Option<std::fs::File>>>,
) {
    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Ok(mut file) = log.lock()
            && let Some(f) = file.as_mut()
        {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
        }
        if let Ok(mut t) = tail.lock() {
            if t.len() == TAIL {
                t.pop_front();
            }
            t.push_back(line);
        }
    }
}

/// Stops a core a previous Kairo left running (after a crash), identified by
/// the pid file and its command line naming this runtime directory.
pub async fn stop_stale(paths: &Paths) {
    let Ok(text) = std::fs::read_to_string(paths.core_pid()) else {
        return;
    };
    let _ = std::fs::remove_file(paths.core_pid());
    let Ok(pid) = text.trim().parse::<i32>() else {
        return;
    };
    if pid <= 1 || !is_our_core(pid, paths) {
        return;
    }
    log::warn!("stopping a core left running by a previous session (pid {pid})");
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
    }
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
    #[cfg(not(target_os = "linux"))]
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
