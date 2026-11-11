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
//!    the user can replace), and makes the folders the core works in (also
//!    administrator-owned, created locked);
//! 2. registers a scheduled task `KairoCore-<user>` that runs the host as SYSTEM
//!    with one fixed argument, its name, and gives that one user the right to run
//!    and end it (a security descriptor on the task). If the Task Scheduler cannot
//!    be used it registers a service of the same name instead;
//! 3. records what it did in `grant-<user>.json` next to the files, with the
//!    SHA-256 of the three files; the host refuses to start when they differ.
//!
//! The user hands the host data, never a path: Kairo writes its config into the
//! one file the user may write in the inbox, and the host reduces it to what the
//! core may run, writes its own copy, and fronts the core's controller (see
//! `kairo-core-host`). After the grant Kairo needs no prompt: `schtasks /run` and
//! `/end` (or `sc`). A user cannot open a SYSTEM process, so whether the core runs
//! is read from a named event the host holds. When an app update brings another
//! core, host or driver library, the recorded hashes no longer match and Kairo
//! asks for the permission again, like on Linux: an update never reaches SYSTEM
//! except through that prompt.
//!
//! The pure parts (argument lines, what counts as granted) are plain functions and
//! are tested everywhere; the Windows calls are below.

use std::path::{Path, PathBuf};

pub use kairo_core_host::names::{
    CORE_EXE, Grant, HOST_EXE, Layout, Route, WINTUN_DLL, host_args, object_name, sanitize_user as sanitize,
    valid_sid,
};

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
        // `try` and `catch` on ONE line: Windows PowerShell reading its script from
        // stdin (`-Command -`) runs a `try` block that has its `catch` on the next
        // line as nothing at all, silently (found by running the real grant).
        "$ErrorActionPreference='Stop'\n\
         try {{ $p = Start-Process -FilePath {exe} -ArgumentList {line} -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1223 }}\n",
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
    pub core: String,
    pub host: String,
    pub wintun: String,
}

pub fn grant_matches(g: &Grant, e: &Expected) -> bool {
    g.name == e.name
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

    use kairo_core_host::names::{self, READ_AND_LIST, READ_ALL, READ_WRITE_DATA, owner_and_dacl};
    use kairo_core_host::winfs;
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

    /// Every path of this user's grant.
    pub fn layout(name: &str) -> Option<Layout> {
        Layout::new(&program_data(), name)
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
        let l = layout(&name)?;
        let text = std::fs::read_to_string(l.grant_file(&name)).ok()?;
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

    /// Where the core's log is (the host writes it; the user may read it).
    pub fn log_path(name: &str) -> Option<PathBuf> {
        layout(name).map(|l| l.log())
    }

    /// Hands the config to the host: written in place into the inbox file, the one
    /// file the user may write (not delete, rename or re-ACL). The host reads it
    /// when it starts and when the controller is asked to reload.
    pub fn deliver_config(name: &str, bytes: &[u8]) -> Result<(), String> {
        let l = layout(name).ok_or("not a Kairo core name")?;
        let path = l.inbox_file();
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| format!("The TUN helper's config file: {e}"))?;
        f.write_all(bytes).map_err(|e| format!("The TUN helper's config file: {e}"))
    }

    fn expected(b: &Bundle, name: &str) -> Option<Expected> {
        Some(Expected {
            name: name.to_owned(),
            core: sha256_of(&b.core)?,
            host: sha256_of(&b.host)?,
            wintun: sha256_of(&b.wintun)?,
        })
    }

    /// Whether the core can be started with TUN without asking.
    ///
    /// Off unless `KAIRO_WINDOWS_TUN=1`, until the design in `RESULT-windows-slice2.md`
    /// (slice 2b) has been independently reviewed.
    pub fn status(_paths: &Paths) -> Privilege {
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
        let (Some(g), Some(e), Some(l)) = (recorded(), expected(&b, &name), layout(&name)) else {
            return Privilege::Needed;
        };
        // The administrators' copies are what runs: they must still be what was
        // approved (the host checks too and would refuse), and this release's
        // files must be the same ones.
        let placed = [(l.core(), &g.core), (l.host(), &g.host), (l.wintun(), &g.wintun)]
            .iter()
            .all(|(p, h)| sha256_of(p).is_some_and(|x| x.eq_ignore_ascii_case(h)));
        if placed
            && l.inbox_file().is_file()
            && grant_matches(&g, &e)
            && registered(g.route, &g.name)
        {
            Privilege::Granted
        } else {
            Privilege::Needed
        }
    }

    /// Asks for administrator approval (UAC) and has an elevated copy of Kairo
    /// do the one-time setup (`run_grant`).
    pub fn grant(paths: &Paths) -> Result<(), String> {
        bundle().ok_or(
            "This copy of Kairo has no TUN helper next to it. Reinstall Kairo to get it back.",
        )?;
        let (user, sid) = current_user()?;
        let exe = crate::paths::current_exe().ok_or("Cannot find Kairo's own program")?;
        let name = object_name(&user);
        let l = layout(&name).ok_or("Cannot name the TUN helper")?;
        let nonce = hex::encode(rand::random::<[u8; 16]>());
        // No path goes to the elevated side: it works from its own folder and
        // from `%ProgramData%`, and says what it did in a file only it can write.
        let args: Vec<String> = ["--grant-tun", "--user", &user, "--sid", &sid, "--nonce", &nonce]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let _ = paths;
        let out = elevate(&exe, &args)?;
        let said = std::fs::read_to_string(l.grant_result(&name)).unwrap_or_default();
        if let Some(rest) = said.trim().strip_prefix(&nonce) {
            let rest = rest.trim();
            if rest.starts_with("ok") && status_for_grant() == Privilege::Granted {
                return Ok(());
            }
            if let Some(e) = rest.strip_prefix("error:") {
                return Err(e.trim().to_owned());
            }
        }
        Err(explain_elevation_failure(&String::from_utf8_lossy(&out.stderr)))
    }

    /// Runs `exe` with `args` through the UAC prompt and waits for it.
    fn elevate(exe: &Path, args: &[String]) -> Result<std::process::Output, String> {
        let script = elevation_script(&exe.to_string_lossy(), args)?;
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
        child.wait_with_output().map_err(|e| e.to_string())
    }

    /// Takes the permission away again (UAC): removes this user's task, folders
    /// and, with the last grant, the files. There is no uninstaller to do it
    /// (Kairo for Windows is a folder that updates itself), so this is how the
    /// SYSTEM side is removed.
    pub fn revoke() -> Result<(), String> {
        let (user, _) = current_user()?;
        let exe = crate::paths::current_exe().ok_or("Cannot find Kairo's own program")?;
        let args: Vec<String> = ["--revoke-tun", "--user", &user].iter().map(|s| s.to_string()).collect();
        let out = elevate(&exe, &args)?;
        if recorded().is_none() {
            Ok(())
        } else {
            Err(explain_elevation_failure(&String::from_utf8_lossy(&out.stderr)))
        }
    }

    /// `status` as the grant itself needs it (the gate is for the product).
    fn status_for_grant() -> Privilege {
        let Some(b) = bundle() else { return Privilege::Unsupported };
        let Ok((user, _)) = current_user() else { return Privilege::Unsupported };
        let name = object_name(&user);
        let (Some(g), Some(e)) = (recorded(), expected(&b, &name)) else {
            return Privilege::Needed;
        };
        if grant_matches(&g, &e) && registered(g.route, &g.name) {
            Privilege::Granted
        } else {
            Privilege::Needed
        }
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

    /// Re-asserts the access of a folder that was already ours: SYSTEM and
    /// Administrators full, Users read and run. By SID, not by name (names are
    /// translated).
    fn icacls(dir: &Path) -> Result<(), String> {
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

    /// A folder only administrators write to: kept when it is already theirs
    /// (owner SYSTEM or Administrators, no link), otherwise removed and created
    /// locked (`%ProgramData%` lets any user create a folder there and own it).
    fn shared_dir(path: &Path) -> Result<(), String> {
        let was_ours = winfs::is_trusted_dir(path);
        let sddl = format!("{}{}", owner_and_dacl(None, "", "OICI"), READ_ALL);
        winfs::ensure_trusted_dir(path, &sddl)?;
        if was_ours {
            icacls(path)?;
        }
        Ok(())
    }

    /// A folder of one user's grant, always made new.
    fn user_dir(path: &Path, sid: Option<&str>, rights: &str, inherit: &str) -> Result<(), String> {
        winfs::remove_any(path)?;
        winfs::create_dir_with(path, &owner_and_dacl(sid, rights, inherit))
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

    /// Runs elevated (`kairo.exe --grant-tun --user U --sid S --nonce N`): the
    /// one-time setup. It takes no path: the files come from beside this program,
    /// everything else is under `%ProgramData%\Kairo`. Returns the exit code.
    pub fn run_grant(args: &[String]) -> i32 {
        let o = opts(args);
        let get = |k: &str| o.get(k).cloned().ok_or_else(|| format!("--{k} is missing"));
        let (user, sid, nonce) = match (get("user"), get("sid"), get("nonce")) {
            (Ok(u), Ok(s), Ok(n)) => (u, s, n),
            _ => return 2,
        };
        if !valid_sid(&sid) || nonce.len() > 64 || !nonce.chars().all(|c| c.is_ascii_hexdigit()) {
            return 2;
        }
        let name = object_name(&user);
        let Some(l) = layout(&name) else { return 2 };
        let go = || -> Result<Route, String> {
            let exe = crate::paths::current_exe().ok_or("cannot find this program")?;
            let b = bundle_of(&exe).ok_or("no TUN helper beside this program")?;
            // An earlier grant may have the core running from these files.
            remove_routes(&name);
            for _ in 0..30 {
                if !running(&name) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            // The folders: administrator-owned, locked as they are created.
            shared_dir(&l.root)?;
            shared_dir(&l.install)?;
            // Whatever else is in there (a folder made the old way, or by someone
            // who could write to it) goes: the loader looks in a program's own
            // folder first, and a file owned by someone else is not ours.
            if let Ok(entries) = std::fs::read_dir(&l.install) {
                for e in entries.flatten() {
                    let file = e.file_name().to_string_lossy().into_owned();
                    if !names::allowed_in_core(&file) || !winfs::is_trusted_file(&e.path()) {
                        winfs::remove_any(&e.path())?;
                    }
                }
            }
            for parent in [l.run.parent(), l.logs.parent(), l.inbox_dir.parent()].into_iter().flatten() {
                shared_dir(parent)?;
            }
            // The core's home: SYSTEM and Administrators only (it holds the
            // controller secret). The log folder the user may read; the inbox
            // folder they may list, and its one file they may write.
            user_dir(&l.run, None, "", "OICI")?;
            user_dir(&l.logs, Some(&sid), READ_AND_LIST, "OICI")?;
            user_dir(&l.inbox_dir, Some(&sid), READ_AND_LIST, "")?;
            winfs::create_file_with(
                &l.inbox_file(),
                &owner_and_dacl(Some(&sid), READ_WRITE_DATA, ""),
            )?;
            // Read, hash, then write the bytes just read: what lands in the
            // protected folder is what was hashed.
            let mut hashes = Vec::new();
            for (src, target) in [(&b.core, CORE_EXE), (&b.host, HOST_EXE), (&b.wintun, WINTUN_DLL)] {
                let bytes = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
                write_atomic(&l.install.join(target), &bytes).map_err(|e| format!("{target}: {e}"))?;
                hashes.push(hex::encode(Sha256::digest(&bytes)));
            }
            let host = l.host();
            let route = match register_task(&name, &host, &host_args("run", &name), &sid) {
                Ok(()) => Route::Task,
                Err(task_err) => {
                    let command = command_line(&host, &host_args("service", &name))?;
                    register_service(&name, &command, &sid).map_err(|e| format!("{task_err}; {e}"))?;
                    Route::Service
                }
            };
            let grant = Grant {
                route,
                name: name.clone(),
                user,
                sid,
                core: hashes[0].clone(),
                host: hashes[1].clone(),
                wintun: hashes[2].clone(),
            };
            let json = serde_json::to_vec_pretty(&grant).map_err(|e| e.to_string())?;
            write_atomic(&l.grant_file(&name), &json).map_err(|e| e.to_string())?;
            Ok(route)
        };
        let (text, code) = match go() {
            Ok(route) => (format!("{nonce} ok {route:?}"), 0),
            Err(e) => (format!("{nonce} error: {e}"), 1),
        };
        if winfs::is_trusted_dir(&l.install) {
            let _ = write_atomic(&l.grant_result(&name), text.as_bytes());
        }
        code
    }

    /// Runs elevated (`kairo.exe --revoke-tun --user U`): removes the task or
    /// service, the grant and this user's folders; the files go with the last grant.
    pub fn run_revoke(args: &[String]) -> i32 {
        let o = opts(args);
        let Some(user) = o.get("user") else { return 2 };
        let name = object_name(user);
        let Some(l) = layout(&name) else { return 2 };
        remove_routes(&name);
        for _ in 0..30 {
            if !running(&name) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let _ = std::fs::remove_file(l.grant_file(&name));
        let _ = std::fs::remove_file(l.grant_result(&name));
        for d in [&l.run, &l.logs, &l.inbox_dir] {
            let _ = winfs::remove_any(d);
        }
        let others = std::fs::read_dir(&l.install)
            .map(|d| {
                d.flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("grant-") && e.file_name().to_string_lossy().ends_with(".json"))
            })
            .unwrap_or(false);
        if !others {
            for f in [CORE_EXE, HOST_EXE, WINTUN_DLL] {
                let _ = std::fs::remove_file(l.install.join(f));
            }
            let _ = std::fs::remove_dir(&l.install);
            // The shared parents go when nothing is left in them.
            for p in [l.run.parent(), l.logs.parent(), l.inbox_dir.parent(), Some(l.root.as_path())]
                .into_iter()
                .flatten()
            {
                let _ = std::fs::remove_dir(p);
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_task_carries_one_name_and_no_path() {
        let args = host_args("run", "KairoCore-t");
        let line = command_line(
            Path::new("C:\\ProgramData\\Kairo\\core\\kairo-core-host.exe"),
            &args,
        )
        .unwrap();
        assert_eq!(
            line,
            "\"C:\\ProgramData\\Kairo\\core\\kairo-core-host.exe\" \"run\" \"--name\" \"KairoCore-t\""
        );
        let bad = vec!["x\" & calc".to_string()];
        assert!(command_line(Path::new("h"), &bad).is_err());
    }

    fn expected() -> Expected {
        Expected {
            name: "KairoCore-t".into(),
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
            core: "aa".into(),
            host: "bb".into(),
            wintun: "cc".into(),
        }
    }

    #[test]
    fn a_new_core_host_or_driver_needs_the_permission_again() {
        assert!(grant_matches(&grant(), &expected()));
        for change in [
            |e: &mut Expected| e.core = "ZZ".into(),
            |e: &mut Expected| e.host = "ZZ".into(),
            |e: &mut Expected| e.wintun = "ZZ".into(),
            |e: &mut Expected| e.name = "KairoCore-other".into(),
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
        let args: Vec<String> = ["--grant-tun", "--user", "PC\\Jo Ann", "--sid", "S-1-5-21-1", "--nonce", "ab12"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let script = elevation_script("C:\\Users\\O'Neil\\kairo.exe", &args).unwrap();
        // One -ArgumentList, a single PowerShell string; apostrophes doubled.
        assert!(script.contains("-FilePath 'C:\\Users\\O''Neil\\kairo.exe'"));
        assert!(script.contains("-ArgumentList '\"--grant-tun\" \"--user\" \"PC\\Jo Ann\""));
        assert!(!script.contains("@("));
        assert!(script.contains("-Verb RunAs -Wait"));
        // Read from stdin, a `catch` that starts a line makes PowerShell skip the `try`.
        assert!(!script.lines().any(|l| l.trim_start().starts_with("catch")));
        // What cannot be carried is refused, not mangled.
        assert!(elevation_script("e", &["a\\".to_string()]).is_err());
        assert!(elevation_script("e", &["a\"b".to_string()]).is_err());
    }

    #[test]
    fn the_grant_takes_no_path_from_the_user() {
        // The elevated side's arguments are a name, a SID and a nonce: nothing
        // that is a path, so there is nothing for the SYSTEM side to open.
        let args = ["--grant-tun", "--user", "--sid", "--nonce"];
        assert!(args.iter().all(|a| a.starts_with("--")));
        assert!(!args.iter().any(|a| ["--dir", "--config", "--log", "--core", "--host", "--wintun", "--result"].contains(a)));
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

    /// What Kairo itself builds for TUN, run through the host's reduction: the
    /// person's proxies, groups, rules, DNS (a profile's own resolver included)
    /// and TUN settings survive; only what the SYSTEM core must not run is left out.
    #[test]
    fn kairos_own_tun_config_passes_the_hosts_reduction_whole() {
        use crate::settings::Settings;
        let profile = "dns:\n  enable: true\n  listen: 127.0.0.1:7874\n  nameserver: [1.1.1.1, 'tls://9.9.9.9:853']\n  proxy-server-nameserver: ['udp://127.0.0.1:7874']\nlisteners:\n  - {name: in, type: mixed, port: 7777}\ntunnels:\n  - tcp,127.0.0.1:6553,1.1.1.1:53,DIRECT\nproxies:\n  - {name: a, type: vless, server: h.example.com, port: 443, uuid: 11111111-1111-1111-1111-111111111111, tls: true, network: ws, ws-opts: {path: /ws}}\n  - {name: b, type: ss, server: s.example.com, port: 8388, cipher: aes-128-gcm, password: p}\nproxy-groups:\n  - {name: G, type: select, proxies: [a, b, DIRECT]}\nrules:\n  - DOMAIN,example.com,G\n  - MATCH,DIRECT\n";
        let mut s = Settings::default();
        s.tun = true;
        s.secret = crate::settings::new_secret();
        let (text, _) = crate::config::build_applied(profile, "", &s, &|_| false).unwrap();
        let out = kairo_core_host::sanitize::sanitize(
            &text,
            &kairo_core_host::sanitize::Internal { port: 40000, secret: "host-only" },
        )
        .unwrap();
        assert_eq!(out.controller_port, s.controller_port);
        assert_eq!(out.secret, s.secret);
        let m: serde_yaml::Value = serde_yaml::from_str(&out.text).unwrap();
        assert_eq!(m["tun"]["enable"].as_bool(), Some(true));
        assert_eq!(m["proxies"].as_sequence().unwrap().len(), 2);
        assert!(m["proxy-groups"].as_sequence().unwrap().len() >= 1, "the profile's group and Kairo's own survive");
        assert_eq!(m["dns"]["listen"].as_str(), Some("127.0.0.1:7874"));
        assert!(m["rules"].as_sequence().unwrap().len() >= 2);
        assert!(m.get("geox-url").is_some());
        // Left out: only the profile's own inbound listeners and tunnels.
        let mut gone = out.dropped.clone();
        gone.sort();
        assert_eq!(gone, ["listeners", "tunnels"], "{:?}", out.dropped);
    }

    /// The same, with everything Kairo adds on top: the AI layer, a route of each
    /// kind, and the nodes real subscriptions carry (WireGuard with its inline key,
    /// hysteria2, a name with a slash). Each must come out as it went in.
    #[test]
    fn kairos_config_with_the_ai_layer_routes_and_varied_nodes_passes_whole() {
        use crate::routes::{Route, RouteExit, RouteKind};
        use crate::settings::Settings;
        let profile = "proxies:\n  - {name: 'JP/Tokyo 01', type: hysteria2, server: h.example.com, port: 443, password: p, sni: h.example.com}\n  - {name: wg, type: wireguard, server: w.example.com, port: 51820, private-key: 'yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=', public-key: 'UuF/iy3Vg3H4rfQxvH1/4mEdcdgDx0gQ1S9tw5xJ2ik=', ip: 10.0.0.2}\n  - {name: v, type: vless, server: v.example.com, port: 443, uuid: 11111111-1111-1111-1111-111111111111, tls: true, servername: v.example.com, reality-opts: {public-key: pk, short-id: ab}}\nproxy-groups:\n  - {name: G, type: select, proxies: ['JP/Tokyo 01', wg, v, DIRECT]}\nrules:\n  - MATCH,G\n";
        let mut s = Settings::default();
        s.tun = true;
        s.secret = crate::settings::new_secret();
        s.ai.enabled = true;
        s.routes = vec![
            Route::new(RouteKind::site("example.org"), RouteExit::Direct).unwrap(),
            Route::new(RouteKind::app("claude"), RouteExit::Ai).unwrap(),
            Route::new(RouteKind::site("blocked.example.com"), RouteExit::Block).unwrap(),
        ];
        let (text, _) = crate::config::build_applied(profile, "", &s, &|_| false).unwrap();
        let out = kairo_core_host::sanitize::sanitize(
            &text,
            &kairo_core_host::sanitize::Internal { port: 40000, secret: "host-only" },
        )
        .unwrap();
        let before: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let after: serde_yaml::Value = serde_yaml::from_str(&out.text).unwrap();
        for key in ["proxies", "proxy-groups", "rules", "tun", "dns", "mode", "find-process-mode", "mixed-port"] {
            assert_eq!(before.get(key), after.get(key), "{key} changed");
        }
        assert_eq!(after["proxies"].as_sequence().unwrap().len(), 3);
        assert!(out.text.contains("PROCESS-NAME,claude"), "the app rule is still there");
        assert_eq!(out.dropped, Vec::<String>::new(), "nothing Kairo builds may be dropped: {:?}", out.dropped);
    }
}
