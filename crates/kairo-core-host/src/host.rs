use std::ffi::OsString;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::Threading::CreateEventW;
use windows::core::{PCWSTR, w};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((mode, rest)) = args.split_first() else {
        eprintln!("usage: kairo-core-host run|service --name N --core C --dir D --config F --log L");
        return 2;
    };
    let opts = match crate::parse(rest) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("kairo-core-host: {e}");
            return 2;
        }
    };
    match mode.as_str() {
        "run" => match run(&opts, None) {
            Ok(code) => code,
            Err(e) => {
                note(&opts, &format!("kairo-core-host: {e}"));
                1
            }
        },
        "service" => {
            *OPTS.lock().unwrap() = Some(opts.clone());
            match service_dispatcher::start(opts["name"].as_str(), ffi_service_main) {
                Ok(()) => 0,
                Err(e) => {
                    note(&opts, &format!("kairo-core-host: service dispatcher: {e}"));
                    1
                }
            }
        }
        other => {
            eprintln!("kairo-core-host: unknown mode {other}");
            2
        }
    }
}

static OPTS: std::sync::Mutex<Option<std::collections::HashMap<String, String>>> =
    std::sync::Mutex::new(None);

/// Best effort: a line into the log the user's Kairo reads for the reason.
fn note(opts: &std::collections::HashMap<String, String>, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&opts["log"]) {
        let _ = writeln!(f, "time=\"-\" level=error msg=\"{line}\"");
    }
}

/// Starts the core, announces itself, waits until the core ends or `stop`
/// (a service's stop request) is signalled. Returns the core's exit code.
fn run(
    opts: &std::collections::HashMap<String, String>,
    stop: Option<std::sync::mpsc::Receiver<()>>,
) -> Result<i32, String> {
    let job = job_that_kills()?;
    let log = std::fs::File::create(&opts["log"]).map_err(|e| format!("log {}: {e}", opts["log"]))?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let mut child: Child = Command::new(&opts["core"])
        .arg("-d")
        .arg(&opts["dir"])
        .arg("-f")
        .arg(&opts["config"])
        .current_dir(&opts["dir"])
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", opts["core"]))?;
    unsafe {
        AssignProcessToJobObject(job, HANDLE(child.as_raw_handle()))
            .map_err(|e| format!("job: {e}"))?;
    }
    // "I am here": opened (never held) by Kairo to ask if the core is running.
    let _announce = announce(&opts["name"]);
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

/// `Global\KairoCoreHost-<name>`, openable by everyone with SYNCHRONIZE.
fn announce(name: &str) -> Option<HANDLE> {
    unsafe {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // Everyone: SYNCHRONIZE only; SYSTEM and administrators: all.
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:(A;;0x100000;;;WD)(A;;GA;;;SY)(A;;GA;;;BA)"),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
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
    let opts = OPTS.lock().unwrap().clone().expect("options");
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let Ok(status_handle) = service_control_handler::register(opts["name"].as_str(), handler) else {
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
    let code = match run(&opts, Some(rx)) {
        Ok(c) => c,
        Err(e) => {
            note(&opts, &format!("kairo-core-host: {e}"));
            1
        }
    };
    let _ = status_handle.set_service_status(status(
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        ServiceExitCode::Win32(if code == 0 { 0 } else { 1 }),
    ));
}
