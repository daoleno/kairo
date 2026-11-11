//! The layout of the SYSTEM side, and its few inputs.
//!
//! ```text
//! %ProgramData%\Kairo                 owner Administrators; SYSTEM, Administrators: full; Users: read
//!   core\                             mihomo.exe, kairo-core-host.exe, wintun.dll, grant-<name>.json
//!   run\<name>\                       the core's home: config.yaml (the host's, holds the controller secret), caches, geo data; the user cannot read it
//!   log\<name>\core.log               what the core and host print; the user may read it
//!   inbox\<name>\config.yaml          the one place the user's Kairo writes (write-data only, no delete)
//! ```
//!
//! Every path the host uses comes from here, derived from `%ProgramData%` and a
//! validated name; none comes from an argument.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CORE_EXE: &str = "mihomo.exe";
pub const HOST_EXE: &str = "kairo-core-host.exe";
pub const WINTUN_DLL: &str = "wintun.dll";
pub const PREFIX: &str = "KairoCore-";

/// How the user starts the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    Task,
    Service,
}

/// What the grant recorded (`grant-<name>.json`, in the administrator-only
/// `core` directory: the host trusts it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub route: Route,
    /// The task or service name; the host's event names end in it.
    pub name: String,
    pub user: String,
    /// The one user who may start and stop the core and talk to it.
    pub sid: String,
    /// SHA-256 (hex) of the files in the `core` directory.
    pub core: String,
    pub host: String,
    pub wintun: String,
}

/// A user name as part of a task, service and object name: letters, digits,
/// `-` and `_` only.
pub fn sanitize_user(user: &str) -> String {
    let short = user.rsplit('\\').next().unwrap_or(user);
    short
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// `KairoCore-<user>`: the task, the service, and (after `KairoCoreHost-`) the event.
pub fn object_name(user: &str) -> String {
    format!("{PREFIX}{}", sanitize_user(user))
}

/// Whether `name` is one `object_name` could have made.
pub fn valid_name(name: &str) -> bool {
    match name.strip_prefix(PREFIX) {
        Some(rest) => {
            !rest.is_empty()
                && rest.len() <= 64
                && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        }
        None => false,
    }
}

/// Whether `sid` is a SID string (`S-1-5-21-…`) and nothing else.
pub fn valid_sid(sid: &str) -> bool {
    sid.starts_with("S-1-")
        && sid.len() <= 184
        && sid.chars().all(|c| c.is_ascii_digit() || c == '-' || c == 'S')
}

/// Every path of one user's grant.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub root: PathBuf,
    pub install: PathBuf,
    pub run: PathBuf,
    pub logs: PathBuf,
    pub inbox_dir: PathBuf,
}

impl Layout {
    pub fn new(program_data: &Path, name: &str) -> Option<Layout> {
        if !valid_name(name) {
            return None;
        }
        let root = program_data.join("Kairo");
        Some(Layout {
            install: root.join("core"),
            run: root.join("run").join(name),
            logs: root.join("log").join(name),
            inbox_dir: root.join("inbox").join(name),
            root,
        })
    }
    pub fn core(&self) -> PathBuf {
        self.install.join(CORE_EXE)
    }
    pub fn host(&self) -> PathBuf {
        self.install.join(HOST_EXE)
    }
    pub fn wintun(&self) -> PathBuf {
        self.install.join(WINTUN_DLL)
    }
    pub fn grant_file(&self, name: &str) -> PathBuf {
        self.install.join(format!("grant-{name}.json"))
    }
    /// What the elevated grant says it did, for the user's Kairo to read.
    pub fn grant_result(&self, name: &str) -> PathBuf {
        self.install.join(format!("grant-result-{name}.txt"))
    }
    /// The user's config, written in place by the user's Kairo.
    pub fn inbox_file(&self) -> PathBuf {
        self.inbox_dir.join("config.yaml")
    }
    /// The config the core is started with: the host's, never the user's.
    pub fn config(&self) -> PathBuf {
        self.run.join("config.yaml")
    }
    pub fn log(&self) -> PathBuf {
        self.logs.join("core.log")
    }
}

/// What the task or service starts the host as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Run,
    Service,
}

/// The host's whole command line: `run|service --name <name>`. Anything else
/// (an extra argument, a path, an unknown flag) is refused.
pub fn parse_host_args(args: &[String]) -> Result<(Mode, String), String> {
    let [mode, flag, name] = args else {
        return Err("usage: kairo-core-host run|service --name KairoCore-<user>".into());
    };
    let mode = match mode.as_str() {
        "run" => Mode::Run,
        "service" => Mode::Service,
        other => return Err(format!("unknown mode {other}")),
    };
    if flag != "--name" {
        return Err(format!("unexpected argument {flag}"));
    }
    if !valid_name(name) {
        return Err("not a Kairo core name".into());
    }
    Ok((mode, name.clone()))
}

/// The arguments the task or service is registered with.
pub fn host_args(mode: &str, name: &str) -> Vec<String> {
    vec![mode.to_owned(), "--name".into(), name.to_owned()]
}

/// Whether `exe_dir` is `<program_data>\Kairo\core` (compared without case).
pub fn is_install_dir(exe_dir: &Path, program_data: &Path) -> bool {
    let want = program_data.join("Kairo").join("core");
    let norm = |p: &Path| {
        p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
    };
    norm(exe_dir) == norm(&want)
}

/// `D:` descriptors, by SID so names and languages do not matter. SYSTEM and
/// Administrators have everything; `user` gets what the second argument names.
pub fn owner_and_dacl(user_sid: Option<&str>, user_rights: &str, inherit: &str) -> String {
    let mut s = format!("O:BAG:BAD:P(A;{inherit};FA;;;SY)(A;{inherit};FA;;;BA)");
    if let Some(sid) = user_sid {
        s.push_str(&format!("(A;{inherit};{user_rights};;;{sid})"));
    }
    s
}

/// What every user may do in `Kairo` and `core`: read and list.
pub const READ_ALL: &str = "(A;OICI;FRFX;;;BU)";
/// The user's rights on a directory of theirs: read and list, nothing that
/// creates, deletes or changes permissions.
pub const READ_AND_LIST: &str = "FRFX";
/// The user's rights on the inbox file: read and write the data (what
/// `GENERIC_READ | GENERIC_WRITE` map to), never delete, rename or re-ACL.
pub const READ_WRITE_DATA: &str = "0x12019f";

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn names_are_safe_to_put_in_a_task_and_an_event() {
        assert_eq!(object_name("WIN-1\\tester"), "KairoCore-tester");
        assert_eq!(object_name("Jo Ann"), "KairoCore-Jo_Ann");
        assert_eq!(object_name("a\"b&c%"), "KairoCore-a_b_c_");
        assert!(valid_name(&object_name("a\\..\\x")));
        assert!(!valid_name("KairoCore-"));
        assert!(!valid_name("KairoCore-..\\x"));
        assert!(!valid_name("evil"));
    }

    #[test]
    fn the_host_takes_one_name_and_nothing_else() {
        let ok = parse_host_args(&s(&["run", "--name", "KairoCore-t"])).unwrap();
        assert_eq!(ok, (Mode::Run, "KairoCore-t".into()));
        assert_eq!(parse_host_args(&s(&["service", "--name", "KairoCore-t"])).unwrap().0, Mode::Service);
        // A path, a core, an extra argument, a bad name: all refused.
        for bad in [
            s(&["run", "--name", "KairoCore-t", "--core", "C:\\evil\\mihomo.exe"]),
            s(&["run", "--core", "C:\\evil\\mihomo.exe", "--name", "KairoCore-t"]),
            s(&["run", "--name", "..\\..\\x"]),
            s(&["run", "--name"]),
            s(&["run"]),
            s(&["exec", "--name", "KairoCore-t"]),
            s(&[]),
        ] {
            assert!(parse_host_args(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn every_path_comes_from_program_data_and_the_name() {
        let l = Layout::new(Path::new("C:\\ProgramData"), "KairoCore-t").unwrap();
        assert_eq!(l.install, Path::new("C:\\ProgramData").join("Kairo").join("core"));
        assert!(l.run.ends_with("run/KairoCore-t") || l.run.ends_with("run\\KairoCore-t"));
        assert!(l.inbox_file().starts_with(&l.root));
        assert!(l.config().starts_with(&l.run) && l.log().starts_with(&l.logs));
        assert!(!l.log().starts_with(&l.run), "the user reads the log, never the core's home");
        assert!(!l.inbox_file().starts_with(&l.run), "the inbox is outside the core's home");
        assert!(Layout::new(Path::new("C:\\ProgramData"), "..\\x").is_none());
    }

    #[test]
    fn the_host_runs_only_from_the_install_directory() {
        let pd = Path::new("C:\\ProgramData");
        assert!(is_install_dir(Path::new("C:\\ProgramData\\Kairo\\core"), pd));
        assert!(is_install_dir(Path::new("c:\\programdata\\kairo\\core\\"), pd));
        assert!(!is_install_dir(Path::new("C:\\Users\\a\\kairo"), pd));
        assert!(!is_install_dir(Path::new("C:\\ProgramData\\Kairo\\core\\x"), pd));
    }

    #[test]
    fn sids_are_only_sids() {
        assert!(valid_sid("S-1-5-21-1-2-3-1002"));
        assert!(!valid_sid("S-1-5-21-1)(A;;FA;;;WD"));
        assert!(!valid_sid("WD"));
    }

    #[test]
    fn the_descriptors_never_give_the_user_delete_or_change_permissions() {
        let d = owner_and_dacl(Some("S-1-5-21-1-2-3-1002"), READ_WRITE_DATA, "");
        assert!(d.starts_with("O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)"));
        assert!(d.ends_with("(A;;0x12019f;;;S-1-5-21-1-2-3-1002)"));
        // 0x12019f has no DELETE (0x10000), WRITE_DAC (0x40000), WRITE_OWNER (0x80000).
        let mask = 0x12019fu32;
        assert_eq!(mask & (0x10000 | 0x40000 | 0x80000), 0);
        assert_eq!(mask & 0x2, 0x2, "write data");
    }
}
