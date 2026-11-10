//! Who holds a local TCP port. Best effort: on Linux from `/proc` (the
//! processes of other users stay unnamed), on macOS from `lsof`, on Windows from
//! the TCP table.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    pub pid: Option<u32>,
    /// The program's name, when it can be seen.
    pub name: Option<String>,
    /// Its command line, when it can be seen.
    pub command: Option<String>,
}

impl Holder {
    /// "verge-mihomo", or a plain phrase when the program cannot be seen.
    pub fn describe(&self) -> String {
        match &self.name {
            Some(n) => n.clone(),
            None => "another program".into(),
        }
    }
}

/// Waits up to `limit` for `port` to be free; whether it is.
pub async fn wait_free(port: u16, limit: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        let in_use = tokio::task::spawn_blocking(move || crate::settings::port_in_use(port))
            .await
            .unwrap_or(true);
        if !in_use {
            return true;
        }
        if start.elapsed() >= limit {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Whether `holder` is a core of the Kairo whose runtime directory is
/// `runtime` (its command line names its config there).
pub fn is_ours(holder: &Holder, runtime: &std::path::Path) -> bool {
    // On Windows a process's command line is not at hand: its executable is,
    // and Kairo runs its core from `<home>/core`.
    #[cfg(windows)]
    if let (Some(command), Some(home)) = (holder.command.as_deref(), runtime.parent())
        && crate::supervisor::same_path(command, &home.join("core").join("mihomo.exe"))
    {
        return true;
    }
    let runtime = runtime.to_string_lossy();
    holder
        .command
        .as_deref()
        .is_some_and(|c| c.contains(runtime.as_ref()))
}

/// Whether a program other than this Kairo's own core holds `port`.
pub fn held_by_other(port: u16, runtime: &std::path::Path) -> bool {
    crate::settings::port_in_use(port) && !holder(port).is_some_and(|h| is_ours(&h, runtime))
}

/// The process listening on TCP `port` on this machine.
pub fn holder(port: u16) -> Option<Holder> {
    #[cfg(target_os = "linux")]
    {
        linux::holder(port)
    }
    #[cfg(target_os = "macos")]
    {
        mac::holder(port)
    }
    #[cfg(windows)]
    {
        windows_holder(port)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = port;
        None
    }
}

#[cfg(windows)]
fn windows_holder(port: u16) -> Option<Holder> {
    use crate::winapi;
    let row = winapi::tcp_table()
        .into_iter()
        .find(|r| r.state == winapi::TCP_LISTEN && r.local.1 == port)?;
    let path = winapi::image_path(row.pid);
    Some(Holder {
        pid: Some(row.pid),
        name: path
            .as_deref()
            .map(|p| winapi::file_name(p).trim_end_matches(".exe").to_owned())
            .or_else(|| {
                winapi::processes()
                    .into_iter()
                    .find(|(pid, _)| *pid == row.pid)
                    .map(|(_, n)| n.trim_end_matches(".exe").to_owned())
            }),
        command: path,
    })
}

/// The listening sockets' inodes for `port` in a `/proc/net/tcp` table.
pub fn listening_inodes(table: &str, port: u16) -> Vec<u64> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let local = f.get(1)?;
            let state = f.get(3)?;
            let p = u16::from_str_radix(local.rsplit(':').next()?, 16).ok()?;
            (p == port && *state == "0A").then(|| f.get(9)?.parse().ok())?
        })
        .collect()
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub fn holder(port: u16) -> Option<Holder> {
        let mut inodes = Vec::new();
        for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
            if let Ok(t) = std::fs::read_to_string(table) {
                inodes.extend(listening_inodes(&t, port));
            }
        }
        if inodes.is_empty() {
            return None;
        }
        let wanted: Vec<String> = inodes.iter().map(|i| format!("socket:[{i}]")).collect();
        for entry in std::fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
                continue;
            };
            let holds = fds.flatten().any(|fd| {
                std::fs::read_link(fd.path())
                    .is_ok_and(|l| wanted.iter().any(|w| l.as_os_str() == w.as_str()))
            });
            if holds {
                let name = std::fs::read_to_string(entry.path().join("comm"))
                    .ok()
                    .map(|n| n.trim().to_owned());
                let command = std::fs::read(entry.path().join("cmdline")).ok().map(|c| {
                    String::from_utf8_lossy(&c)
                        .replace('\0', " ")
                        .trim()
                        .to_owned()
                });
                return Some(Holder {
                    pid: Some(pid),
                    name,
                    command,
                });
            }
        }
        // Listening, but in a process this user cannot look into.
        Some(Holder {
            pid: None,
            name: None,
            command: None,
        })
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    pub fn holder(port: u16) -> Option<Holder> {
        let out = std::process::Command::new("/usr/sbin/lsof")
            // `+c 0`: the whole command name, not lsof's first nine characters.
            .args([
                "-nP",
                "+c",
                "0",
                &format!("-iTCP:{port}"),
                "-sTCP:LISTEN",
                "-Fpc",
            ])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let pid = text
            .lines()
            .find_map(|l| l.strip_prefix('p'))
            .and_then(|p| p.parse::<u32>().ok());
        let name = text
            .lines()
            .find_map(|l| l.strip_prefix('c'))
            .map(str::to_owned);
        let command = pid.and_then(|p| {
            std::process::Command::new("ps")
                .args(["-p", &p.to_string(), "-o", "command="])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        });
        if pid.is_none() && name.is_none() {
            // lsof cannot see other users' processes (a root service).
            return crate::settings::port_in_use(port).then_some(Holder {
                pid: None,
                name: None,
                command: None,
            });
        }
        Some(Holder { pid, name, command })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_proc_net_tcp() {
        let t = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1ED9 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000000000000000 100 0 0 10 0\n   1: 0100007F:1ED9 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 4343 1 0000000000000000 20 4 30 10 -1\n";
        assert_eq!(listening_inodes(t, 7897), vec![4242]);
        assert!(listening_inodes(t, 7890).is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn names_its_own_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = holder(port).unwrap();
        assert_eq!(h.pid, Some(std::process::id()));
        assert!(h.name.is_some());
        drop(l);
        // The kernel lets go of a closed listener at once, but another test
        // of this process may bind and drop its own sockets meanwhile: look a
        // few times before calling it held.
        let free = (0..20).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            holder(port).is_none()
        });
        assert!(free, "a closed listener is no holder");
    }
}
