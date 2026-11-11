use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kairo_core_host::gate::{self, MAX_BODY, Verdict};
use kairo_core_host::names::{self, Grant, Layout, Mode};
use kairo_core_host::sanitize::{self, Internal};
use kairo_core_host::winfs;
use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::Threading::{
    CreateEventW, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::{PCWSTR, PWSTR};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The biggest config the host reads from the user.
const MAX_CONFIG: u64 = 16 * 1024 * 1024;

pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mode, name) = match names::parse_host_args(&args) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("kairo-core-host: {e}");
            return 2;
        }
    };
    match mode {
        Mode::Run => match run(&name, None) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("kairo-core-host: {e}");
                1
            }
        },
        Mode::Service => {
            *NAME.lock().unwrap() = Some(name.clone());
            match service_dispatcher::start(name.as_str(), ffi_service_main) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("kairo-core-host: service dispatcher: {e}");
                    1
                }
            }
        }
    }
}

static NAME: Mutex<Option<String>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A line into the log the user's Kairo reads for the reason.
type Log = Arc<Mutex<std::fs::File>>;

fn note(log: &Log, line: &str) {
    let _ = writeln!(lock(log), "time=\"-\" level=info msg=\"kairo-core-host: {line}\"");
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// Why the host did not start, into the log the user can read (when the run
/// folder is the administrators' own).
fn fail_note(name: &str, why: &str) {
    let Some(pd) = std::env::var_os("ProgramData") else { return };
    let Some(layout) = Layout::new(Path::new(&pd), name) else { return };
    if !winfs::is_trusted_dir(&layout.logs) {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(layout.log()) {
        let _ = writeln!(f, "time=\"-\" level=error msg=\"kairo-core-host: {why}\"");
    }
}

/// Everything the host checked before it starts the core.
struct Ready {
    layout: Layout,
    grant: Grant,
    log: Log,
    internal_port: u16,
    internal_secret: String,
    user_port: u16,
    user_secret: String,
    dropped: Vec<String>,
}

fn prepare(name: &str) -> Result<Ready, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let program_data = std::env::var_os("ProgramData")
        .map(std::path::PathBuf::from)
        .ok_or("no ProgramData")?;
    let layout = Layout::new(&program_data, name).ok_or("not a Kairo core name")?;
    // This program must be the administrator-owned copy.
    let dir = exe.parent().ok_or("no program directory")?;
    if !names::is_install_dir(dir, &program_data) {
        return Err(format!("not running from {}", layout.install.display()));
    }
    for d in [&layout.root, &layout.install, &layout.run, &layout.logs, &layout.inbox_dir] {
        if !winfs::is_trusted_dir(d) {
            return Err(format!("{} is not an administrator-owned folder", d.display()));
        }
    }
    // The log first, so the rest can say why it failed.
    let file = std::fs::File::create(layout.log()).map_err(|e| format!("log: {e}"))?;
    let log: Log = Arc::new(Mutex::new(file));

    let grant_bytes = winfs::read_plain_file(&layout.grant_file(name), 64 * 1024)?;
    let grant: Grant = serde_json::from_slice(&grant_bytes).map_err(|e| format!("grant: {e}"))?;
    if grant.name != name || !names::valid_sid(&grant.sid) {
        return Err("the grant is not this name's".into());
    }
    // What runs is what the administrator copied.
    for (what, path, want) in [
        ("the core", layout.core(), &grant.core),
        ("wintun.dll", layout.wintun(), &grant.wintun),
        ("the host", exe.clone(), &grant.host),
    ] {
        let got = sha256_file(&path)?;
        if !got.eq_ignore_ascii_case(want) {
            return Err(format!("{what} is not the one that was approved; give Kairo the permission again"));
        }
    }

    let internal_secret = hex::encode(rand::random::<[u8; 32]>());
    let internal_port = free_port()?;
    let text = winfs::read_plain_file(&layout.inbox_file(), MAX_CONFIG)?;
    let text = String::from_utf8(text).map_err(|_| "the config is not UTF-8".to_owned())?;
    let s = sanitize::sanitize(&text, &Internal { port: internal_port, secret: &internal_secret })?;
    std::fs::write(layout.config(), &s.text).map_err(|e| format!("config: {e}"))?;
    Ok(Ready {
        layout,
        grant,
        log,
        internal_port,
        internal_secret,
        user_port: s.controller_port,
        user_secret: s.secret,
        dropped: s.dropped,
    })
}

/// A loopback port nothing listens on now (the core takes it a moment later).
fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

/// What the connections share.
struct Shared {
    layout: Layout,
    sid: String,
    log: Log,
    internal_port: u16,
    internal_secret: String,
    user_port: u16,
    user_secret: Mutex<String>,
    reload: Mutex<()>,
}

/// Starts the core and its controller front, announces itself, waits until the
/// core ends or `stop` is signalled. Returns the core's exit code.
fn run(name: &str, stop: Option<std::sync::mpsc::Receiver<()>>) -> Result<i32, String> {
    let ready = prepare(name).inspect_err(|e| fail_note(name, e))?;
    let log = ready.log.clone();
    for d in &ready.dropped {
        note(&log, &format!("left out of the config: {d}"));
    }
    let shared = Arc::new(Shared {
        layout: ready.layout.clone(),
        sid: ready.grant.sid.clone(),
        log: log.clone(),
        internal_port: ready.internal_port,
        internal_secret: ready.internal_secret.clone(),
        user_port: ready.user_port,
        user_secret: Mutex::new(ready.user_secret.clone()),
        reload: Mutex::new(()),
    });
    let listener = TcpListener::bind(("127.0.0.1", ready.user_port))
        .map_err(|e| format!("controller port {}: {e}", ready.user_port))?;
    let job = job_that_kills()?;
    let (out, err) = {
        let f = lock(&log);
        (f.try_clone().map_err(|e| e.to_string())?, f.try_clone().map_err(|e| e.to_string())?)
    };
    let run_dir = &ready.layout.run;
    let tmp = run_dir.join("tmp");
    let _ = std::fs::create_dir_all(&tmp);
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    // The core gets a clean environment: nothing the user could have set, no
    // SAFE_PATHS or SKIP_SAFE_PATH_CHECK.
    let mut child: Child = Command::new(ready.layout.core())
        .arg("-d")
        .arg(run_dir)
        .arg("-f")
        .arg(ready.layout.config())
        .current_dir(run_dir)
        .env_clear()
        .env("SystemRoot", &system_root)
        .env("windir", &system_root)
        .env("SystemDrive", std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()))
        .env("PATH", format!("{system_root}\\System32"))
        .env("TEMP", &tmp)
        .env("TMP", &tmp)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", ready.layout.core().display()))?;
    unsafe {
        AssignProcessToJobObject(job, HANDLE(child.as_raw_handle()))
            .map_err(|e| format!("job: {e}"))?;
    }
    {
        let shared = shared.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let shared = shared.clone();
                std::thread::spawn(move || serve(conn, &shared));
            }
        });
    }
    // "I am here": opened (never held) by Kairo to ask if the core is running.
    let _announce = announce(name, &ready.grant.sid);
    note(&log, &format!("core started; controller answers on 127.0.0.1:{}", ready.user_port));
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(status.code().unwrap_or(1));
        }
        match &stop {
            Some(rx) => {
                if rx.recv_timeout(Duration::from_millis(300)).is_ok() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(0);
                }
            }
            None => std::thread::sleep(Duration::from_millis(300)),
        }
    }
}

// --- The controller front ---------------------------------------------------------

fn reply(c: &mut TcpStream, status: u16, text: &str, body: &str) {
    let _ = write!(
        c,
        "HTTP/1.1 {status} {text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn message(m: &str) -> String {
    serde_json::json!({ "message": m }).to_string()
}

fn serve(mut c: TcpStream, sh: &Shared) {
    let _ = c.set_read_timeout(Some(Duration::from_secs(10)));
    // Only the installing user's processes: the connection's owner is looked up.
    match peer_sid(&c, sh.user_port) {
        Some(sid) if sid.eq_ignore_ascii_case(&sh.sid) => {}
        _ => return reply(&mut c, 403, "Forbidden", &message("not the user this core was set up for")),
    }
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head = loop {
        match gate::parse_head(&buf) {
            Ok(Some(h)) => break h,
            Ok(None) if buf.len() < 16 * 1024 => {}
            _ => return reply(&mut c, 400, "Bad Request", &message("bad request")),
        }
        match c.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let secret = lock(&sh.user_secret).clone();
    if !gate::authorized(&head, &secret) {
        return reply(&mut c, 401, "Unauthorized", &message("Unauthorized"));
    }
    if head.header("transfer-encoding").is_some() {
        return reply(&mut c, 400, "Bad Request", &message("bad request"));
    }
    let want: usize = head.header("content-length").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    if want > MAX_BODY {
        return reply(&mut c, 413, "Payload Too Large", &message("too large"));
    }
    let mut body = buf[head.len..].to_vec();
    while body.len() < want {
        match c.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    body.truncate(want);
    match gate::decide(&head.method, &head.target, &body) {
        Verdict::Deny(why) => {
            note(&sh.log, &format!("refused {} {} ({why})", head.method, head.target));
            reply(&mut c, 403, "Forbidden", &message("Kairo's TUN helper does not do that"));
        }
        Verdict::Reload => {
            let (status, text) = reload(sh);
            reply(&mut c, status, if status == 204 || status == 200 { "OK" } else { "Bad Request" }, &text);
        }
        Verdict::Forward => forward(c, sh, &head, &body),
    }
}

/// Re-reads the inbox, sanitizes it, and has the core load the host's own file.
fn reload(sh: &Shared) -> (u16, String) {
    let _one = lock(&sh.reload);
    let go = || -> Result<(u16, String), String> {
        let text = winfs::read_plain_file(&sh.layout.inbox_file(), MAX_CONFIG)?;
        let text = String::from_utf8(text).map_err(|_| "the config is not UTF-8".to_owned())?;
        let s = sanitize::sanitize(&text, &Internal { port: sh.internal_port, secret: &sh.internal_secret })?;
        if s.controller_port != sh.user_port {
            return Err("the controller port changed; turn TUN off and on again".into());
        }
        std::fs::write(sh.layout.config(), &s.text).map_err(|e| format!("config: {e}"))?;
        *lock(&sh.user_secret) = s.secret;
        for d in &s.dropped {
            note(&sh.log, &format!("left out of the config: {d}"));
        }
        let body = serde_json::json!({ "path": sh.layout.config().to_string_lossy(), "payload": "" }).to_string();
        let req = format!(
            "PUT /configs?force=true HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            sh.internal_port,
            sh.internal_secret,
            body.len()
        );
        let mut up = TcpStream::connect(("127.0.0.1", sh.internal_port)).map_err(|e| e.to_string())?;
        let _ = up.set_read_timeout(Some(Duration::from_secs(90)));
        up.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        let mut resp = Vec::new();
        up.read_to_end(&mut resp).map_err(|e| e.to_string())?;
        let status = std::str::from_utf8(&resp[..resp.len().min(16)])
            .ok()
            .and_then(|s| s.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(502);
        let at = resp.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(resp.len());
        Ok((status, String::from_utf8_lossy(&resp[at..]).into_owned()))
    };
    match go() {
        Ok((status, body)) => (status, body),
        Err(e) => {
            note(&sh.log, &format!("reload: {e}"));
            (400, message(&e))
        }
    }
}

fn forward(mut c: TcpStream, sh: &Shared, head: &gate::Head, body: &[u8]) {
    let Ok(mut up) = TcpStream::connect(("127.0.0.1", sh.internal_port)) else {
        return reply(&mut c, 502, "Bad Gateway", &message("the core is not answering yet"));
    };
    let req = gate::forward_head(head, sh.internal_port, &sh.internal_secret, body.len());
    if up.write_all(req.as_bytes()).is_err() || up.write_all(body).is_err() {
        return;
    }
    let _ = c.set_read_timeout(None);
    let (Ok(mut c2), Ok(mut up2)) = (c.try_clone(), up.try_clone()) else { return };
    // The user's side only matters for a WebSocket (it has nothing more to say
    // after a plain request); ending either side ends both.
    let t = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = c2.read(&mut buf) {
            if n == 0 || up2.write_all(&buf[..n]).is_err() {
                break;
            }
        }
        let _ = up2.shutdown(Shutdown::Write);
    });
    let mut buf = [0u8; 8192];
    while let Ok(n) = up.read(&mut buf) {
        if n == 0 || c.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    let _ = c.shutdown(Shutdown::Both);
    let _ = up.shutdown(Shutdown::Both);
    let _ = t.join();
}

/// The SID of the user who owns the process at the other end of `c`.
fn peer_sid(c: &TcpStream, server_port: u16) -> Option<String> {
    let client_port = c.peer_addr().ok()?.port();
    if !c.peer_addr().ok()?.ip().is_loopback() {
        return None;
    }
    let pid = unsafe {
        let mut size = 0u32;
        let _ = GetExtendedTcpTable(None, &mut size, false, 2, TCP_TABLE_OWNER_PID_ALL, 0);
        let mut buf = vec![0u8; size as usize + 4096];
        size = buf.len() as u32;
        let rc = GetExtendedTcpTable(
            Some(buf.as_mut_ptr() as *mut _),
            &mut size,
            false,
            2,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );
        if rc != 0 {
            return None;
        }
        let count = u32::from_ne_bytes(buf[..4].try_into().ok()?) as usize;
        let rows = buf.as_ptr().add(4) as *const MIB_TCPROW_OWNER_PID;
        let port = |p: u32| u16::from_be((p & 0xffff) as u16);
        (0..count)
            .map(|i| std::ptr::read_unaligned(rows.add(i)))
            .find(|r| port(r.dwLocalPort) == client_port && port(r.dwRemotePort) == server_port)
            .map(|r| r.dwOwningPid)?
    };
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut token = HANDLE::default();
        let sid = if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_ok() {
            let mut len = 0u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
            let mut info = vec![0u8; len as usize];
            let out = if GetTokenInformation(token, TokenUser, Some(info.as_mut_ptr() as *mut _), len, &mut len)
                .is_ok()
            {
                let user = &*(info.as_ptr() as *const TOKEN_USER);
                let mut s = PWSTR::null();
                let r = ConvertSidToStringSidW(user.User.Sid, &mut s);
                let t = r.ok().map(|_| s.to_string().unwrap_or_default());
                if !s.is_null() {
                    let _ = LocalFree(Some(HLOCAL(s.0 as _)));
                }
                t
            } else {
                None
            };
            let _ = CloseHandle(token);
            out
        } else {
            None
        };
        let _ = CloseHandle(process);
        sid
    }
}

// --- Process plumbing -------------------------------------------------------------

/// A job object whose processes end when this process (its only handle) does.
fn job_that_kills() -> Result<HANDLE, String> {
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null()).map_err(|e| format!("job: {e}"))?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .map_err(|e| format!("job limit: {e}"))?;
        Ok(job)
    }
}

/// `Global\KairoCoreHost-<name>`, openable (to wait on) by the installing user,
/// SYSTEM and administrators only.
fn announce(name: &str, sid: &str) -> Option<HANDLE> {
    unsafe {
        let sddl: Vec<u16> = format!("D:(A;;0x100000;;;{sid})(A;;GA;;;SY)(A;;GA;;;BA)\0").encode_utf16().collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut descriptor, None)
            .ok()?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let full: Vec<u16> = format!("Global\\KairoCoreHost-{name}\0").encode_utf16().collect();
        let event = CreateEventW(Some(&attributes), true, false, PCWSTR(full.as_ptr()));
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        event.ok()
    }
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    let Some(name) = lock(&NAME).clone() else { return };
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let Ok(status_handle) = service_control_handler::register(name.as_str(), handler) else {
        return;
    };
    let status = |state, accept, code| ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: code,
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    let _ = status_handle.set_service_status(status(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ServiceExitCode::Win32(0),
    ));
    let code = match run(&name, Some(rx)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("kairo-core-host: {e}");
            1
        }
    };
    let _ = status_handle.set_service_status(status(
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        ServiceExitCode::Win32(if code == 0 { 0 } else { 1 }),
    ));
}
