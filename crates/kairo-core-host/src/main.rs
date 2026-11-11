//! `kairo-core-host`: what Windows runs, as SYSTEM, to give the mihomo core the
//! rights TUN needs (a wintun adapter, routes).
//!
//! ```text
//! kairo-core-host run     --name N --core mihomo.exe --dir D --config C --log L
//! kairo-core-host service --name N --core mihomo.exe --dir D --config C --log L
//! ```
//!
//! `run` is the action of the scheduled task; `service` is the same under the
//! service control manager, for machines where the task cannot be used. Both:
//! start the core in a job object that dies with this process (ending the task or
//! stopping the service ends the core, nothing is left behind), send its output to
//! the log file, and hold a named event, `Global\KairoCoreHost-<name>`, that anyone
//! may open to ask "is it running?" (a user cannot open a SYSTEM process).
//!
//! The arguments are fixed when the permission is given (by an administrator), in
//! the task or service definition; nothing here reads configuration from a place
//! a user can write, other than the core's own config file.

#[cfg(windows)]
mod host;

#[cfg(windows)]
fn main() {
    std::process::exit(host::main());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("kairo-core-host runs on Windows only");
    std::process::exit(1);
}

/// The arguments after the mode: `--key value` pairs.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse(args: &[String]) -> Result<std::collections::HashMap<String, String>, String> {
    let mut out = std::collections::HashMap::new();
    let mut it = args.iter();
    while let Some(k) = it.next() {
        let key = k.strip_prefix("--").ok_or_else(|| format!("unexpected argument {k}"))?;
        let value = it.next().ok_or_else(|| format!("--{key} needs a value"))?;
        out.insert(key.to_owned(), value.clone());
    }
    for need in ["name", "core", "dir", "config", "log"] {
        if !out.contains_key(need) {
            return Err(format!("--{need} is missing"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_pairs_and_wants_all_of_them() {
        let a = s(&["--name", "n", "--core", "c", "--dir", "d", "--config", "f", "--log", "l"]);
        assert_eq!(parse(&a).unwrap()["core"], "c");
        assert!(parse(&s(&["--name", "n"])).unwrap_err().contains("--core"));
        assert!(parse(&s(&["x"])).is_err());
        assert!(parse(&s(&["--name"])).is_err());
    }
}
