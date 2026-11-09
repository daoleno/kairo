//! Which process owns a local connection, when the core cannot say.
//!
//! mihomo names a connection's process by reading the system's socket tables.
//! On a macOS where that read no longer works (0.2.0 was checked on macOS 27:
//! every connection came back without a name) the Activity page would show
//! "Unknown" for every tool. For connections from this computer, `lsof` still
//! knows the owner of each socket, as it does for the program holding a port
//! (see `ports`), so Kairo asks it, a few times a second at most, only when a
//! connection came without a name. Only the command name is read: no
//! arguments, no paths, nothing about what the process sends.

use std::collections::HashMap;

/// Whether Kairo should ask `lsof` itself: on macOS, or when
/// `KAIRO_LSOF_PROCESS=1` says so (a test of the plumbing on Linux).
pub fn enabled() -> bool {
    cfg!(target_os = "macos") || std::env::var_os("KAIRO_LSOF_PROCESS").is_some_and(|v| v == "1")
}

/// The local port of an `n` line of `lsof -F`: `127.0.0.1:52235->1.2.3.4:443`
/// or `[::1]:52235->[::1]:7897`. Listening sockets (no `->`) are not
/// connections and give `None`.
fn local_port(name: &str) -> Option<u16> {
    let (local, _) = name.split_once("->")?;
    local.rsplit_once(':')?.1.parse().ok()
}

/// The process (its id and command name) of each local port in `lsof -nP +c 0
/// -iTCP -Fpcn` output. Sockets of the core itself appear too, under the
/// proxy's own port: they are harmless, no connection starts there.
pub fn parse(text: &str) -> HashMap<u16, (u32, String)> {
    let mut out = HashMap::new();
    let mut pid = 0u32;
    let mut command: Option<&str> = None;
    for line in text.lines() {
        let Some(rest) = line.get(1..) else { continue };
        match line.as_bytes()[0] {
            // A new process: forget the previous one's name until its own.
            b'p' => {
                command = None;
                pid = rest.parse().unwrap_or(0);
            }
            b'c' => command = Some(rest),
            b'n' => {
                if let (Some(c), Some(port)) = (command, local_port(rest)) {
                    out.entry(port).or_insert_with(|| (pid, c.to_owned()));
                }
            }
            _ => {}
        }
    }
    out
}

/// Runs `lsof` and returns the owner of every established local socket.
/// Blocking, so call it off the async threads.
pub fn lookup() -> HashMap<u16, (u32, String)> {
    let lsof = ["/usr/sbin/lsof", "/usr/bin/lsof"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());
    let Some(lsof) = lsof else {
        return HashMap::new();
    };
    std::process::Command::new(lsof)
        // `+c 0`: the whole command name, not lsof's first nine characters.
        .args(["-nP", "+c", "0", "-iTCP", "-sTCP:ESTABLISHED", "-Fpcn"])
        .output()
        .map(|o| parse(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// What Kairo remembers between snapshots of the connections.
#[derive(Debug, Default)]
pub struct Names {
    /// Owner by local port, as of the last look.
    ports: HashMap<u16, (u32, String)>,
    /// When the last look ran (unix seconds).
    at: i64,
}

impl Names {
    /// The owner of a local port: its process id and command name.
    pub fn get(&self, port: u16) -> Option<(u32, &str)> {
        self.ports.get(&port).map(|(p, n)| (*p, n.as_str()))
    }

    /// Whether a new look is due: something is unnamed that the last look
    /// did not know, at most once a second.
    pub fn due(&self, now: i64, unnamed: impl IntoIterator<Item = u16>) -> bool {
        now > self.at && unnamed.into_iter().any(|p| !self.ports.contains_key(&p))
    }

    pub fn replace(&mut self, ports: HashMap<u16, (u32, String)>, now: i64) {
        self.ports = ports;
        self.at = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "p615\ncCursor Helper (Renderer)\nn127.0.0.1:52235->127.0.0.1:27898\nn[::1]:52300->[::1]:27898\np700\ncmihomo\nn127.0.0.1:27898->127.0.0.1:52235\nn*:27898\np812\nccurl\nn127.0.0.1:52411->127.0.0.1:27898\n";

    #[test]
    fn reads_the_owner_of_each_local_port() {
        let m = parse(SAMPLE);
        let own = |p: u16| m.get(&p).map(|(i, n)| (*i, n.as_str()));
        assert_eq!(own(52235), Some((615, "Cursor Helper (Renderer)")));
        assert_eq!(own(52300), Some((615, "Cursor Helper (Renderer)")));
        assert_eq!(own(52411), Some((812, "curl")));
        // The core's own end sits under its own port, and a listener has none.
        assert_eq!(own(27898), Some((700, "mihomo")));
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn a_name_never_carries_over_to_the_next_process() {
        let m =
            parse("p1\nn127.0.0.1:1000->1.1.1.1:443\np2\ncgood\nn127.0.0.1:2000->1.1.1.1:443\n");
        assert!(!m.contains_key(&1000));
        assert_eq!(
            m.get(&2000).map(|(i, n)| (*i, n.as_str())),
            Some((2, "good"))
        );
    }

    #[test]
    fn looks_again_only_for_what_it_does_not_know_and_once_a_second() {
        let mut n = Names::default();
        assert!(n.due(10, [5000]));
        n.replace(parse(SAMPLE), 10);
        assert!(!n.due(10, [52235]), "known");
        assert!(!n.due(10, [9999]), "not within the same second");
        assert!(n.due(11, [9999]), "unknown, a second later");
        assert!(!n.due(11, [52235]));
    }

    #[test]
    fn a_real_lsof_names_this_process_when_lsof_is_there() {
        if !std::path::Path::new("/usr/bin/lsof").exists()
            && !std::path::Path::new("/usr/sbin/lsof").exists()
        {
            return;
        }
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let c = std::net::TcpStream::connect(addr).unwrap();
        let _s = l.accept().unwrap();
        let port = c.local_addr().unwrap().port();
        let m = lookup();
        let (pid, name) = m.get(&port).expect("lsof sees this socket");
        assert_eq!(*pid, std::process::id());
        assert!(!name.is_empty());
    }
}
