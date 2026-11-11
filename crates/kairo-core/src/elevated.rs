//! TUN on Windows: the core runs as SYSTEM, and the user starts and stops it.
//!
//! Windows has no file capability or setuid bit to give a user's own copy of the
//! core, and a standard account cannot create a network adapter. So, once, with
//! administrator approval (the UAC prompt, which asks a standard user for an
//! administrator's credentials), Kairo
//!
//! 1. copies the core (`mihomo.exe`), the wintun driver library (`wintun.dll`) and
//!    a small host (`kairo-core-host.exe`) to `%ProgramData%\Kairo\core`, which
//!    only administrators can write to (so what runs as SYSTEM is never a file
//!    the user can replace);
//! 2. registers a scheduled task `KairoCore-<user>` that runs the host as SYSTEM
//!    with the arguments fixed (core, runtime directory, config, log), and gives
//!    that one user the right to run and end it (a security descriptor on the
//!    task). If the Task Scheduler cannot be used it registers a service of the
//!    same name instead, with the same right to start and stop it;
//! 3. records what it did in `grant-<user>.json` next to the files (readable by
//!    all), with the SHA-256 of the three files.
//!
//! After that Kairo needs no prompt: `schtasks /run` and `schtasks /end` (or
//! `sc start`/`sc stop`). A user cannot open a SYSTEM process (not even to ask if
//! it is alive), so whether the core runs is read from a named event the host
//! holds, `Global\KairoCoreHost-<name>`. When an app update brings another core,
//! host or driver library, the recorded hashes no longer match and Kairo asks for
//! the permission again, like on Linux.
//!
//! The pure parts (names, descriptors, argument lines, what counts as granted)
//! are plain functions and are tested everywhere; the Windows calls are below.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CORE_EXE: &str = "mihomo.exe";
pub const HOST_EXE: &str = "kairo-core-host.exe";
pub const WINTUN_DLL: &str = "wintun.dll";

/// How the user starts the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    Task,
    Service,
}

/// What the grant recorded (`grant-<name>.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub route: Route,
    /// The task or service name, and the host's event name suffix.
    pub name: String,
    pub user: String,
    pub sid: String,
    /// The runtime directory, config and log the core is started with.
    pub dir: String,
    pub config: String,
    pub log: String,
    /// SHA-256 (hex) of the files in `%ProgramData%\Kairo\core`.
    pub core: String,
    pub host: String,
    pub wintun: String,
}

/// The files Kairo ships for TUN, next to its executable.
#[derive(Debug, Clone, PartialEq)]
pub struct Bundle {
    pub core: PathBuf,
    pub host: PathBuf,
    pub wintun: PathBuf,
}

/// The bundle beside `exe`, when all three files are there.
pub fn bundle_of(exe: &Path) -> Option<Bundle> {
    let dir = exe.parent()?;
    let b = Bundle {
        core: dir.join(CORE_EXE),
        host: dir.join(HOST_EXE),
        wintun: dir.join(WINTUN_DLL),
    };
    (b.core.is_file() && b.host.is_file() && b.wintun.is_file()).then_some(b)
}

/// A user name as part of a task, service and object name: letters, digits,
/// `-` and `_` only.
pub fn sanitize(user: &str) -> String {
    let short = user.rsplit('\\').next().unwrap_or(user);
    short
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// `KairoCore-<user>`: the task, the service, and (after `KairoCoreHost-`) the event.
pub fn object_name(user: &str) -> String {
    format!("KairoCore-{}", sanitize(user))
}

/// Where the permission's files live, below `%ProgramData%`.
pub fn install_dir_in(program_data: &Path) -> PathBuf {
    program_data.join("Kairo").join("core")
}

pub fn grant_file_in(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("grant-{name}.json"))
}

/// The arguments the host is started with, fixed in the task or service.
pub fn host_args(
    mode: &str,
    name: &str,
    install: &Path,
    dir: &str,
    config: &str,
    log: &str,
) -> Vec<String> {
    vec![
        mode.to_owned(),
        "--name".into(),
        name.to_owned(),
        "--core".into(),
        install.join(CORE_EXE).to_string_lossy().into_owned(),
        "--dir".into(),
        dir.to_owned(),
        "--config".into(),
        config.to_owned(),
        "--log".into(),
        log.to_owned(),
    ]
}

/// Arguments as one string: each in double quotes (no Windows path can hold one).
pub fn quote_args(args: &[String]) -> Result<String, String> {
    let mut parts = Vec::new();
    for a in args {
        // A quote, a line break, or a backslash right before the closing quote
        // (which would escape it) cannot be carried in a Windows command line.
        if a.contains('"') || a.contains('\0') || a.contains('\n') || a.ends_with('\\') {
            return Err(format!("cannot pass {a:?} to the core host"));
        }
        parts.push(format!("\"{a}\""));
    }
    Ok(parts.join(" "))
}

/// `'text'` for PowerShell: single quotes inside it doubled.
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The PowerShell that starts `exe` elevated (the UAC prompt) with `args` and
/// waits. Windows PowerShell 5.1 joins an `-ArgumentList` array with spaces and
/// quotes nothing, so the arguments go as one string, quoted here.
pub fn elevation_script(exe: &str, args: &[String]) -> Result<String, String> {
    let line = quote_args(args)?;
    Ok(format!(
        "$ErrorActionPreference='Stop'\n\
         try {{ $p = Start-Process -FilePath {exe} -ArgumentList {line} -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode }}\n\
         catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1223 }}\n",
        exe = ps_quote(exe),
        line = ps_quote(&line),
    ))
}

/// A command line: the program and its arguments, each in double quotes.
pub fn command_line(program: &Path, args: &[String]) -> Result<String, String> {
    Ok(format!("\"{}\" {}", program.display(), quote_args(args)?))
}

/// The task's security descriptor: SYSTEM and administrators everything, and
/// this user the right to read, run and end it.
pub fn task_sddl(sid: &str) -> String {
    format!("D:(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGX;;;{sid})")
}

/// The service's: SYSTEM and administrators as usual, and this user may query,
/// start, stop and interrogate it.
pub fn service_sddl(sid: &str) -> String {
    format!(
        "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;LCRPWPLORC;;;{sid})"
    )
}

/// `"DOMAIN\\user","S-1-5-..."` (the output of `whoami /user /fo csv /nh`).
pub fn parse_whoami(csv: &str) -> Option<(String, String)> {
    let line = csv.lines().find(|l| !l.trim().is_empty())?;
    let mut it = line.trim().trim_matches('"').split("\",\"");
    let user = it.next()?.to_owned();
    let sid = it.next()?.to_owned();
    (sid.starts_with("S-1-")).then_some((user, sid))
}

/// What a recorded grant has to equal for the core to start without asking.
#[derive(Debug, Clone, PartialEq)]
pub struct Expected {
    pub name: String,
    pub dir: String,
    pub config: String,
    pub log: String,
    pub core: String,
    pub host: String,
    pub wintun: String,
}

pub fn grant_matches(g: &Grant, e: &Expected) -> bool {
    g.name == e.name
        && g.dir == e.dir
        && g.config == e.config
        && g.log == e.log
        && g.core.eq_ignore_ascii_case(&e.core)
        && g.host.eq_ignore_ascii_case(&e.host)
        && g.wintun.eq_ignore_ascii_case(&e.wintun)
}

/// `Cancelled` for what Windows says when the user closes the UAC prompt.
pub fn explain_elevation_failure(stderr: &str) -> String {
    let s = stderr.trim();
    let l = s.to_ascii_lowercase();
    if l.contains("canceled by the user") || l.contains("cancelled by the user") || l.contains("1223") {
        "Cancelled".into()
    } else if s.is_empty() {
        "Windows did not give Kairo the permission".into()
    } else {
        s.lines().next().unwrap_or(s).to_owned()
    }
}

#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
mod win {
    use std::collections::HashMap;
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::{Mutex, OnceLock};
    use std::time::SystemTime;

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::paths::{Paths, write_atomic};
    use crate::tun::Privilege;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn quiet(program: &str) -> Command {
        let mut c = Command::new(program);
        c.creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .stdout(Stdio::piped());
        c
    }

    fn system32(exe: &str) -> PathBuf {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(root).join("System32").join(exe)
    }

    pub fn program_data() -> PathBuf {
        std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
    }

    pub fn install_dir() -> PathBuf {
        install_dir_in(&program_data())
    }

    /// The signed-in user's name and SID.
    pub fn current_user() -> Result<(String, String), String> {
        static USER: OnceLock<Option<(String, String)>> = OnceLock::new();
        USER.get_or_init(|| {
            let out = quiet(&system32("whoami.exe").to_string_lossy())
                .args(["/user", "/fo", "csv", "/nh"])
                .output()
                .ok()?;
            parse_whoami(&String::from_utf8_lossy(&out.stdout))
        })
        .clone()
        .ok_or_else(|| "Could not tell which Windows user this is".to_owned())
    }

    pub fn bundle() -> Option<Bundle> {
        bundle_of(&crate::paths::current_exe()?)
    }

    fn sha256_of(path: &Path) -> Option<String> {
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, (u64, SystemTime, String)>>> = OnceLock::new();
        let meta = std::fs::metadata(path).ok()?;
        let key = (meta.len(), meta.modified().ok()?);
        let cache = CACHE.get_or_init(Default::default);
        if let Some((len, time, hash)) = cache.lock().ok()?.get(path)
            && (*len, *time) == key
        {
            return Some(hash.clone());
        }
        let bytes = std::fs::read(path).ok()?;
        let hash = hex::encode(Sha256::digest(&bytes));
        cache
            .lock()
            .ok()?
            .insert(path.to_path_buf(), (key.0, key.1, hash.clone()));
        Some(hash)
    }

    /// The grant on record for this user, if any.
    pub fn recorded() -> Option<Grant> {
        let (user, _) = current_user().ok()?;
        let name = object_name(&user);
        let text = std::fs::read_to_string(grant_file_in(&install_dir(), &name)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Whether the task or service exists (the grant file alone is not enough:
    /// someone may have deleted the task).
    fn registered(route: Route, name: &str) -> bool {
        let out = match route {
            Route::Task => quiet(&system32("schtasks.exe").to_string_lossy())
                .args(["/query", "/tn", name])
                .output(),
            Route::Service => quiet(&system32("sc.exe").to_string_lossy())
                .args(["query", name])
                .output(),
        };
        out.map(|o| o.status.success()).unwrap_or(false)
    }

    /// Whether the core host runs: its event can be opened (and is gone when it is not).
    pub fn running(name: &str) -> bool {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenEventW, SYNCHRONIZATION_SYNCHRONIZE};
        use windows::core::PCWSTR;
        let full: Vec<u16> = format!("Global\\KairoCoreHost-{name}\0").encode_utf16().collect();
        unsafe {
            match OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(full.as_ptr())) {
                Ok(h) => {
                    let _ = CloseHandle(h);
                    true
                }
                Err(_) => false,
            }
        }
    }

    /// Starts the task or service (no prompt: the grant gave this user the right).
    pub fn start(route: Route, name: &str) -> Result<(), String> {
        let out = match route {
            Route::Task => quiet(&system32("schtasks.exe").to_string_lossy())
                .args(["/run", "/tn", name])
                .output(),
            Route::Service => quiet(&system32("sc.exe").to_string_lossy())
                .args(["start", name])
                .output(),
        }
        .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            let text = String::from_utf8_lossy(if out.stderr.is_empty() { &out.stdout } else { &out.stderr })
                .trim()
                .to_owned();
            Err(format!("Windows would not start the core: {text}"))
        }
    }

    /// Ends the task or stops the service; the core goes with it.
    pub fn stop(route: Route, name: &str) {
        let _ = match route {
            Route::Task => quiet(&system32("schtasks.exe").to_string_lossy())
                .args(["/end", "/tn", name])
                .output(),
            Route::Service => quiet(&system32("sc.exe").to_string_lossy())
                .args(["stop", name])
                .output(),
        };
    }

    /// The route and name this user's core is started with, when it can be.
    pub fn current() -> Option<(Route, String)> {
        let g = recorded()?;
        registered(g.route, &g.name).then_some((g.route, g.name))
    }

    fn expected(paths: &Paths, b: &Bundle, name: &str) -> Option<Expected> {
        Some(Expected {
            name: name.to_owned(),
            dir: paths.runtime_dir().to_string_lossy().into_owned(),
            config: paths.runtime_config().to_string_lossy().into_owned(),
            log: paths.logs_dir().join("core.log").to_string_lossy().into_owned(),
            core: sha256_of(&b.core)?,
            host: sha256_of(&b.host)?,
            wintun: sha256_of(&b.wintun)?,
        })
    }

    /// Whether the core can be started with TUN without asking.
    ///
    /// Off unless `KAIRO_WINDOWS_TUN=1`: the SYSTEM host still opens files under
    /// folders a standard user controls (the runtime and logs folders), which a
    /// directory junction can redirect. Until the host works only in an
    /// administrator-owned folder this stays a test switch.
    pub fn status(paths: &Paths) -> Privilege {
        if std::env::var_os("KAIRO_WINDOWS_TUN").is_none_or(|v| v != "1") {
            return Privilege::Unsupported;
        }
        let Some(b) = bundle() else {
            return Privilege::Unsupported;
        };
        let Ok((user, _)) = current_user() else {
            return Privilege::Unsupported;
        };
        let name = object_name(&user);
        let (Some(g), Some(e)) = (recorded(), expected(paths, &b, &name)) else {
            return Privilege::Needed;
        };
        let dir = install_dir();
        let placed = [CORE_EXE, HOST_EXE, WINTUN_DLL]
            .iter()
            .all(|f| dir.join(f).is_file());
        if placed && grant_matches(&g, &e) && registered(g.route, &g.name) {
            Privilege::Granted
        } else {
            Privilege::Needed
        }
    }

    /// Asks for administrator approval (UAC) and has an elevated copy of Kairo
    /// do the one-time setup (`run_grant`).
    pub fn grant(paths: &Paths) -> Result<(), String> {
        let b = bundle().ok_or(
            "This copy of Kairo has no TUN helper next to it. Reinstall Kairo to get it back.",
        )?;
        let (user, sid) = current_user()?;
        let exe = crate::paths::current_exe().ok_or("Cannot find Kairo's own program")?;
        let result = paths.state_dir().join("tun-grant-result.txt");
        let _ = std::fs::create_dir_all(paths.state_dir());
        let _ = std::fs::remove_file(&result);
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let args: Vec<String> = vec![
            "--grant-tun".into(),
            "--user".into(),
            user,
            "--sid".into(),
            sid,
            "--dir".into(),
            s(&paths.runtime_dir()),
            "--config".into(),
            s(&paths.runtime_config()),
            "--log".into(),
            s(&paths.logs_dir().join("core.log")),
            "--core".into(),
            s(&b.core),
            "--host".into(),
            s(&b.host),
            "--wintun".into(),
            s(&b.wintun),
            "--result".into(),
            s(&result),
        ];
        let script = elevation_script(&s(&exe), &args)?;
        let ps = system32("WindowsPowerShell\\v1.0\\powershell.exe");
        let mut child = quiet(&ps.to_string_lossy())
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", "-"])
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| format!("powershell: {e}"))?;
        child
            .stdin
            .take()
            .ok_or("powershell: no input")?
            .write_all(script.as_bytes())
            .map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let said = std::fs::read_to_string(&result).unwrap_or_default();
        let said = said.trim();
        if said.starts_with("ok") && status(paths) == Privilege::Granted {
            return Ok(());
        }
        if let Some(e) = said.strip_prefix("error:") {
            return Err(e.trim().to_owned());
        }
        Err(explain_elevation_failure(&String::from_utf8_lossy(&out.stderr)))
    }

    // --- The elevated side ------------------------------------------------------

    fn opts(args: &[String]) -> HashMap<String, String> {
        let mut m = HashMap::new();
        let mut it = args.iter();
        while let Some(k) = it.next() {
            if let Some(key) = k.strip_prefix("--")
                && let Some(v) = it.next()
            {
                m.insert(key.to_owned(), v.clone());
            }
        }
        m
    }

    fn icacls(dir: &Path) -> Result<(), String> {
        // SYSTEM, Administrators: full; Users: read and run. By SID, not by name
        // (names are translated).
        let out = quiet(&system32("icacls.exe").to_string_lossy())
            .arg(dir)
            .args([
                "/inheritance:r",
                "/grant:r",
                "*S-1-5-18:(OI)(CI)F",
                "*S-1-5-32-544:(OI)(CI)F",
                "*S-1-5-32-545:(OI)(CI)RX",
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("icacls: {}", String::from_utf8_lossy(&out.stdout).trim()))
        }
    }

    fn register_task(name: &str, host: &Path, args: &[String], sid: &str) -> Result<(), String> {
        let script = "\
$ErrorActionPreference = 'Stop'\n\
$a = New-ScheduledTaskAction -Execute $env:K_HOST -Argument $env:K_ARGS\n\
$p = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest\n\
$s = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew\n\
Register-ScheduledTask -TaskName $env:K_TASK -Action $a -Principal $p -Settings $s -Force | Out-Null\n\
$svc = New-Object -ComObject Schedule.Service\n\
$svc.Connect()\n\
$t = $svc.GetFolder('\\').GetTask($env:K_TASK)\n\
$t.SetSecurityDescriptor($env:K_SDDL, 0)\n";
        let arguments = quote_args(args)?;
        let ps = system32("WindowsPowerShell\\v1.0\\powershell.exe");
        let mut child = quiet(&ps.to_string_lossy())
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", "-"])
            .env("K_HOST", host)
            .env("K_ARGS", arguments)
            .env("K_TASK", name)
            .env("K_SDDL", task_sddl(sid))
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child
            .stdin
            .take()
            .ok_or("no input")?
            .write_all(script.as_bytes())
            .map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() && registered(Route::Task, name) {
            Ok(())
        } else {
            Err(format!(
                "scheduled task: {}",
                String::from_utf8_lossy(&out.stderr).trim().lines().next().unwrap_or("failed")
            ))
        }
    }

    fn register_service(name: &str, command: &str, sid: &str) -> Result<(), String> {
        let sc = system32("sc.exe");
        let run = |args: &[&str]| {
            quiet(&sc.to_string_lossy())
                .args(args)
                .output()
                .map_err(|e| e.to_string())
        };
        let _ = run(&["delete", name]);
        let out = run(&[
            "create",
            name,
            "binPath=",
            command,
            "start=",
            "demand",
            "obj=",
            "LocalSystem",
            "DisplayName=",
            &format!("Kairo core ({name})"),
        ])?;
        if !out.status.success() {
            return Err(format!("service: {}", String::from_utf8_lossy(&out.stdout).trim()));
        }
        let out = run(&["sdset", name, &service_sddl(sid)])?;
        if !out.status.success() {
            return Err(format!("service rights: {}", String::from_utf8_lossy(&out.stdout).trim()));
        }
        Ok(())
    }

    fn remove_routes(name: &str) {
        stop(Route::Task, name);
        stop(Route::Service, name);
        let _ = quiet(&system32("schtasks.exe").to_string_lossy())
            .args(["/delete", "/tn", name, "/f"])
            .output();
        let _ = quiet(&system32("sc.exe").to_string_lossy())
            .args(["delete", name])
            .output();
    }

    /// Runs elevated (`kairo.exe --grant-tun ...`): the one-time setup. Writes the
    /// outcome to `--result`. Returns the process exit code.
    pub fn run_grant(args: &[String]) -> i32 {
        let o = opts(args);
        let result_file = o.get("result").cloned();
        let report = |text: String, code: i32| {
            if let Some(p) = &result_file {
                let _ = std::fs::write(p, &text);
            }
            code
        };
        let get = |k: &str| o.get(k).cloned().ok_or_else(|| format!("--{k} is missing"));
        let go = || -> Result<Route, String> {
            let (user, sid) = (get("user")?, get("sid")?);
            if !sid.starts_with("S-1-") || sid.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
                return Err("not a user SID".into());
            }
            let (dir, config, log) = (get("dir")?, get("config")?, get("log")?);
            let name = object_name(&user);
            let install = install_dir();
            std::fs::create_dir_all(&install).map_err(|e| format!("{}: {e}", install.display()))?;
            // An earlier grant may have the core running from these files.
            remove_routes(&name);
            for _ in 0..30 {
                if !running(&name) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            // Read, hash, then write the bytes just read: what lands in the
            // protected directory is what was hashed.
            let mut hashes = Vec::new();
            for (key, target) in [("core", CORE_EXE), ("host", HOST_EXE), ("wintun", WINTUN_DLL)] {
                let src = get(key)?;
                let bytes = std::fs::read(&src).map_err(|e| format!("{src}: {e}"))?;
                write_atomic(&install.join(target), &bytes)
                    .map_err(|e| format!("{target}: {e}"))?;
                hashes.push(hex::encode(Sha256::digest(&bytes)));
            }
            icacls(&install)?;
            let host = install.join(HOST_EXE);
            let task_args = host_args("run", &name, &install, &dir, &config, &log);
            let route = match register_task(&name, &host, &task_args, &sid) {
                Ok(()) => Route::Task,
                Err(task_err) => {
                    let svc_args = host_args("service", &name, &install, &dir, &config, &log);
                    let command = command_line(&host, &svc_args)?;
                    register_service(&name, &command, &sid)
                        .map_err(|e| format!("{task_err}; {e}"))?;
                    Route::Service
                }
            };
            let grant = Grant {
                route,
                name: name.clone(),
                user,
                sid,
                dir,
                config,
                log,
                core: hashes[0].clone(),
                host: hashes[1].clone(),
                wintun: hashes[2].clone(),
            };
            let json = serde_json::to_vec_pretty(&grant).map_err(|e| e.to_string())?;
            write_atomic(&grant_file_in(&install, &name), &json).map_err(|e| e.to_string())?;
            Ok(route)
        };
        match go() {
            Ok(route) => report(format!("ok {route:?}"), 0),
            Err(e) => report(format!("error: {e}"), 1),
        }
    }

    /// Runs elevated (`kairo.exe --revoke-tun --user U`): removes the task or
    /// service and the grant; the files go with the last grant.
    pub fn run_revoke(args: &[String]) -> i32 {
        let o = opts(args);
        let Some(user) = o.get("user") else { return 2 };
        let name = object_name(user);
        remove_routes(&name);
        let install = install_dir();
        let _ = std::fs::remove_file(grant_file_in(&install, &name));
        let others = std::fs::read_dir(&install)
            .map(|d| {
                d.flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("grant-"))
            })
            .unwrap_or(false);
        if !others {
            for f in [CORE_EXE, HOST_EXE, WINTUN_DLL] {
                let _ = std::fs::remove_file(install.join(f));
            }
            let _ = std::fs::remove_dir(&install);
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_to_put_in_a_task_and_an_event() {
        assert_eq!(object_name("WIN-1\\tester"), "KairoCore-tester");
        assert_eq!(object_name("Jo Ann"), "KairoCore-Jo_Ann");
        assert_eq!(object_name("a\"b&c%"), "KairoCore-a_b_c_");
        assert_eq!(sanitize("DOMAIN\\ma.ria"), "ma_ria");
    }

    #[test]
    fn the_descriptors_give_one_user_the_right_to_start_and_stop() {
        let sid = "S-1-5-21-1-2-3-1002";
        assert_eq!(
            task_sddl(sid),
            "D:(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGX;;;S-1-5-21-1-2-3-1002)"
        );
        let s = service_sddl(sid);
        assert!(s.ends_with("(A;;LCRPWPLORC;;;S-1-5-21-1-2-3-1002)"));
        // Start (RP), stop (WP), query status (LC); never delete (SD) or change
        // the configuration (DC, WD, WO) for the user.
        let user = s.rsplit('(').next().unwrap();
        assert!(user.contains("RP") && user.contains("WP") && user.contains("LC"));
        assert!(!user.contains("SD") && !user.contains("WD") && !user.contains("WO"));
    }

    #[test]
    fn whoami_is_read_in_any_language() {
        let (u, s) = parse_whoami("\"win-0de24353pkg\\kstd\",\"S-1-5-21-1-2-3-1002\"\r\n").unwrap();
        assert_eq!(u, "win-0de24353pkg\\kstd");
        assert_eq!(s, "S-1-5-21-1-2-3-1002");
        assert!(parse_whoami("ERROR: nope").is_none());
        assert!(parse_whoami("").is_none());
    }

    #[test]
    fn the_host_is_started_with_fixed_quoted_arguments() {
        let args = host_args(
            "run",
            "KairoCore-t",
            Path::new("C:\\ProgramData\\Kairo\\core"),
            "C:\\Users\\a b\\AppData\\Roaming\\Kairo\\runtime",
            "C:\\Users\\a b\\AppData\\Roaming\\Kairo\\runtime\\config.yaml",
            "C:\\Users\\a b\\AppData\\Roaming\\Kairo\\logs\\core.log",
        );
        let line = command_line(Path::new("C:\\ProgramData\\Kairo\\core\\kairo-core-host.exe"), &args).unwrap();
        assert!(line.starts_with("\"C:\\ProgramData\\Kairo\\core\\kairo-core-host.exe\" \"run\" \"--name\""));
        assert!(line.contains("\"C:\\Users\\a b\\AppData\\Roaming\\Kairo\\runtime\""));
        let bad = vec!["x\" & calc".to_string()];
        assert!(command_line(Path::new("h"), &bad).is_err());
    }

    fn expected() -> Expected {
        Expected {
            name: "KairoCore-t".into(),
            dir: "d".into(),
            config: "c".into(),
            log: "l".into(),
            core: "AA".into(),
            host: "BB".into(),
            wintun: "CC".into(),
        }
    }

    fn grant() -> Grant {
        Grant {
            route: Route::Task,
            name: "KairoCore-t".into(),
            user: "u".into(),
            sid: "S-1-5-21-1".into(),
            dir: "d".into(),
            config: "c".into(),
            log: "l".into(),
            core: "aa".into(),
            host: "bb".into(),
            wintun: "cc".into(),
        }
    }

    #[test]
    fn a_new_core_host_driver_or_place_needs_the_permission_again() {
        assert!(grant_matches(&grant(), &expected()));
        for change in [
            |e: &mut Expected| e.core = "ZZ".into(),
            |e: &mut Expected| e.host = "ZZ".into(),
            |e: &mut Expected| e.wintun = "ZZ".into(),
            |e: &mut Expected| e.dir = "other".into(),
            |e: &mut Expected| e.log = "other".into(),
        ] {
            let mut e = expected();
            change(&mut e);
            assert!(!grant_matches(&grant(), &e));
        }
    }

    #[test]
    fn the_grant_survives_a_round_trip_through_its_file() {
        let g = grant();
        let json = serde_json::to_string(&g).unwrap();
        assert!(json.contains("\"route\":\"task\""));
        assert_eq!(serde_json::from_str::<Grant>(&json).unwrap(), g);
    }

    #[test]
    fn closing_the_prompt_reads_as_cancelled() {
        assert_eq!(explain_elevation_failure("The operation was canceled by the user.\r\n"), "Cancelled");
        assert_eq!(explain_elevation_failure(""), "Windows did not give Kairo the permission");
        assert_eq!(explain_elevation_failure("boom\nmore"), "boom");
    }

    #[test]
    fn the_elevation_script_carries_spaces_and_apostrophes_as_one_quoted_line() {
        let args: Vec<String> = [
            "--grant-tun",
            "--user",
            "PC\\Jo Ann",
            "--dir",
            "C:\\Users\\Jo Ann\\AppData\\Roaming\\Kairo\\runtime",
            "--log",
            "C:\\Users\\O'Neil\\Kairo\\logs\\core.log",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let script = elevation_script("C:\\Users\\O'Neil\\kairo.exe", &args).unwrap();
        // One -ArgumentList, a single PowerShell string; apostrophes doubled.
        assert!(script.contains("-FilePath 'C:\\Users\\O''Neil\\kairo.exe'"));
        assert!(script.contains("-ArgumentList '\"--grant-tun\" \"--user\" \"PC\\Jo Ann\""));
        assert!(script.contains("\"C:\\Users\\O''Neil\\Kairo\\logs\\core.log\"'"));
        assert!(!script.contains("@("));
        assert!(script.contains("-Verb RunAs -Wait"));
        // What cannot be carried is refused, not mangled.
        assert!(elevation_script("e", &["a\\".to_string()]).is_err());
        assert!(elevation_script("e", &["a\"b".to_string()]).is_err());
    }

    #[test]
    fn the_bundle_needs_all_three_files() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("kairo.exe");
        std::fs::write(&exe, b"x").unwrap();
        assert!(bundle_of(&exe).is_none());
        for f in [CORE_EXE, HOST_EXE, WINTUN_DLL] {
            std::fs::write(tmp.path().join(f), b"x").unwrap();
        }
        assert_eq!(bundle_of(&exe).unwrap().host, tmp.path().join(HOST_EXE));
    }
}
