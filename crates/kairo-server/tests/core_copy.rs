//! The core in Kairo's folder across a start of a newly installed server: a
//! copy with the shipped core's bytes is never touched (its TUN permission,
//! inode and times stay), a mihomo beside the server is not its core, only
//! the pinned version replaces the copy, and a replacement is said in
//! `status --json`.
//!
//! The server runs from a scratch prefix laid out as install.sh lays it out
//! (`<prefix>/bin/kairo-server`, the shipped core in `<prefix>/lib/kairo`).
//! With `KAIRO_TEST_CAPS=1` (scripts/tun-e2e.sh, in a user namespace) the
//! copy also carries the file capability `setcap` gives it, and the server
//! runs without CAP_NET_ADMIN in its bounding set.

#![cfg(target_os = "linux")]

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

const MARKER: &std::ffi::CStr = c"user.kairo_marker";

/// Writes of files that are run, kept apart from forks. The tests run in
/// threads of one process. A child forked while one of them has a file open
/// for writing holds that open until it execs, and an exec of the file in
/// that window fails with "Text file busy" (ETXTBSY: CI saw the server's own
/// copy refuse to start). So a write of anything that is run takes this
/// lock alone, and every spawn shares it: no file that is run is open for
/// writing while this process forks (as Go's `syscall.ForkLock`).
static FORK: RwLock<()> = RwLock::new(());

/// Writes a file that will be run: no fork happens while it is open.
fn writing<T>(write: impl FnOnce() -> T) -> T {
    let _alone = FORK.write().unwrap_or_else(PoisonError::into_inner);
    write()
}

/// Every child of these tests starts here. `spawn` returns once the child
/// has exec'd, so the descriptors it was forked with are closed by then.
fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    let _shared = FORK.read().unwrap_or_else(PoisonError::into_inner);
    cmd.spawn()
}

/// [`Command::output`], started through [`spawn`].
fn output(cmd: &mut Command) -> std::io::Result<Output> {
    spawn(cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()))?.wait_with_output()
}

/// A scratch install: the server and its core in `prefix`, Kairo's folder in
/// `home` (geo data in place, nothing started by itself), a page in `page`.
struct Place {
    dir: tempfile::TempDir,
}

impl Place {
    fn new(shipped_core: &[u8]) -> Place {
        let dir = tempfile::tempdir().unwrap();
        let p = Place { dir };
        std::fs::create_dir_all(p.path("prefix/bin")).unwrap();
        std::fs::create_dir_all(p.path("prefix/lib/kairo")).unwrap();
        writing(|| std::fs::copy(env!("CARGO_BIN_EXE_kairo-server"), p.server())).unwrap();
        write_exe(&p.shipped(), shipped_core);
        let runtime = p.path("home/runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        for name in ["geoip.metadb", "GeoSite.dat", "Country.mmdb", "GeoIP.dat"] {
            std::fs::File::create(runtime.join(name)).unwrap().set_len(1024 * 1024).unwrap();
        }
        std::fs::write(p.path("home/settings.json"), r#"{"autoStartCore":false}"#).unwrap();
        std::fs::create_dir_all(p.path("page")).unwrap();
        std::fs::write(p.path("page/index.html"), "<!doctype html><title>Kairo</title>").unwrap();
        std::fs::create_dir_all(p.path("home/core")).unwrap();
        p
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn server(&self) -> PathBuf {
        self.path("prefix/bin/kairo-server")
    }
    fn shipped(&self) -> PathBuf {
        self.path("prefix/lib/kairo/mihomo")
    }
    fn copy(&self) -> PathBuf {
        self.path("home/core/mihomo")
    }

    /// Starts the server, through `wrap` (a command and its arguments that
    /// end by running the rest) when given, and returns `status --json` once
    /// its start has looked at the core.
    fn start(&self, wrap: &[&str]) -> (Child, Value) {
        let mut cmd = match wrap.split_first() {
            Some((first, rest)) => {
                let mut c = Command::new(first);
                c.args(rest).arg(self.server());
                c
            }
            None => Command::new(self.server()),
        };
        let mut child = spawn(
            cmd.args(["serve", "--listen", "127.0.0.1:0", "--web"])
                .arg(self.path("page"))
                .env("KAIRO_HOME", self.path("home"))
                .env("KAIRO_NO_SYSPROXY", "1")
                .env("KAIRO_NO_UPDATE", "1")
                .env("KAIRO_NO_ICON_FETCH", "1")
                .env("RUST_LOG", "kairo_core=info")
                .stdout(Stdio::null())
                .stderr(Stdio::inherit()),
        )
        .expect("kairo-server runs");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let out = output(Command::new(self.server()).args(["status", "--json"]).env("KAIRO_HOME", self.path("home"))).unwrap();
            let s: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
            // `tun_permission` is set by the same step that looked at the core.
            if s["running"] == true && !s["tun_permission"].is_null() {
                return (child, s);
            }
            if let Ok(Some(code)) = child.try_wait() {
                panic!("the server stopped: {code}");
            }
            assert!(Instant::now() < deadline, "the server did not come up: {s}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn stop(mut child: Child) {
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn write_exe(path: &Path, bytes: &[u8]) {
    writing(|| std::fs::write(path, bytes)).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A stand-in core: answers `-v` as mihomo does.
fn fake_core(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho Mihomo Meta {version} linux amd64 with go1.26.10\n").into_bytes()
}

fn set_marker(path: &Path) {
    let p = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let v = b"seen-before";
    let r = unsafe { libc::setxattr(p.as_ptr(), MARKER.as_ptr(), v.as_ptr().cast(), v.len(), 0) };
    assert_eq!(r, 0, "user xattrs on the scratch folder: {}", std::io::Error::last_os_error());
}

fn xattr(path: &Path, name: &std::ffi::CStr) -> Option<Vec<u8>> {
    let p = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let mut buf = [0u8; 256];
    let n = unsafe { libc::getxattr(p.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    (n >= 0).then(|| buf[..n as usize].to_vec())
}

/// What a start must leave as it was.
#[derive(Debug, PartialEq)]
struct Untouched {
    ino: u64,
    mtime: (i64, i64),
    marker: Option<Vec<u8>>,
    capability: Option<Vec<u8>>,
}

fn untouched(path: &Path) -> Untouched {
    let m = std::fs::metadata(path).unwrap();
    Untouched {
        ino: m.ino(),
        mtime: (m.mtime(), m.mtime_nsec()),
        marker: xattr(path, MARKER),
        capability: xattr(path, c"security.capability"),
    }
}

/// The folder's copy: `bytes`, written an hour before the shipped core (an
/// older mtime than the seed), with a marker.
fn older_copy(place: &Place, bytes: &[u8]) {
    write_exe(&place.copy(), bytes);
    older_copy_times_and_marker(place);
}

fn pinned() -> &'static str {
    kairo_core::core_bin::pinned_version()
}

#[test]
fn a_copy_with_the_shipped_cores_bytes_is_left_alone_however_new_the_seed() {
    let core = fake_core(pinned());
    let place = Place::new(&core);
    older_copy(&place, &core);
    let before = untouched(&place.copy());
    assert!(
        std::fs::metadata(place.shipped()).unwrap().modified().unwrap() > std::fs::metadata(place.copy()).unwrap().modified().unwrap()
    );

    let (child, status) = place.start(&[]);
    stop(child);
    assert_eq!(untouched(&place.copy()), before, "the copy was rewritten");
    assert!(status["core_replaced"].is_null(), "{status}");
    assert_eq!(status["core_version"], pinned(), "{status}");
}

/// The cutover's case (its evidence, scenario B): the server installed into
/// `~/.local`, where the machine's own, older mihomo sits in `bin` beside it.
/// That one is not Kairo's: the folder's core keeps its inode, mtime and
/// marker, with the package's seed there or not.
#[test]
fn a_different_mihomo_beside_the_server_is_not_its_core() {
    let core = fake_core(pinned());
    let place = Place::new(&core);
    write_exe(&place.path("prefix/bin/mihomo"), &fake_core("v1.19.18"));
    older_copy(&place, &core);
    let before = untouched(&place.copy());

    let (child, status) = place.start(&[]);
    stop(child);
    assert_eq!(untouched(&place.copy()), before, "the mihomo beside the server replaced the folder's core");
    assert!(status["core_replaced"].is_null(), "{status}");
    assert_eq!(status["core_version"], pinned(), "{status}");

    // Without the package's own core (lib/kairo), still not that one.
    std::fs::remove_file(place.shipped()).unwrap();
    let (child, status) = place.start(&[]);
    stop(child);
    assert_eq!(untouched(&place.copy()), before, "the mihomo beside the server replaced the folder's core");
    assert_eq!(status["core_version"], pinned(), "{status}");
}

/// A core in the package's place that is not the pinned one (or is older
/// than the folder's) does not replace the folder's.
#[test]
fn a_core_kairo_does_not_ship_never_replaces_the_folders() {
    let place = Place::new(&fake_core("v1.19.18"));
    older_copy(&place, &fake_core(pinned()));
    let before = untouched(&place.copy());
    let (child, status) = place.start(&[]);
    stop(child);
    assert_eq!(untouched(&place.copy()), before);
    assert!(status["core_replaced"].is_null(), "{status}");
    assert_eq!(status["core_version"], pinned(), "{status}");
}

#[test]
fn an_update_replaces_the_copy_and_status_says_so() {
    let place = Place::new(&fake_core(pinned()));
    older_copy(&place, &fake_core("v1.0.0"));
    let before = untouched(&place.copy());

    let (child, status) = place.start(&[]);
    stop(child);
    assert_ne!(untouched(&place.copy()).ino, before.ino, "the copy was not replaced");
    assert_eq!(std::fs::read(place.copy()).unwrap(), fake_core(pinned()));
    assert_eq!(status["core_replaced"]["from"], "v1.0.0", "{status}");
    assert_eq!(status["core_replaced"]["to"], pinned(), "{status}");
    // Root (as in a user namespace) runs TUN without a grant.
    let root = unsafe { libc::geteuid() } == 0;
    assert_eq!(status["core_replaced"]["tun_permission_needed"], !root, "{status}");
    assert_eq!(status["tun_permission"], if root { "granted" } else { "needed" }, "{status}");

    // The next start finds it in place: nothing replaced any more.
    let after = untouched(&place.copy());
    let (child, status) = place.start(&[]);
    stop(child);
    assert_eq!(untouched(&place.copy()), after);
    assert!(status["core_replaced"].is_null(), "{status}");
}

/// The real core, granted (a file capability), the same bytes as the
/// shipped one, and a server that may not exec it (no CAP_NET_ADMIN in its
/// bounding set: exec fails with EPERM, so asking the copy its version
/// fails, which once was enough to replace it). Needs a user namespace (`unshare -r`), where
/// `setcap` and `capsh` work without root: scripts/tun-e2e.sh.
#[test]
fn a_granted_copy_that_this_process_cannot_run_keeps_its_permission() {
    if std::env::var_os("KAIRO_TEST_CAPS").is_none() {
        eprintln!("skipped: KAIRO_TEST_CAPS=1 (scripts/tun-e2e.sh runs it in a user namespace)");
        return;
    }
    let core = std::fs::read(std::env::var("KAIRO_TEST_CORE").expect("KAIRO_TEST_CORE: the mihomo core")).unwrap();
    let place = Place::new(&core);
    write_exe(&place.copy(), &core);
    let st = spawn(Command::new("setcap").args(["cap_net_admin,cap_net_bind_service=+ep"]).arg(place.copy()))
        .and_then(|mut c| c.wait())
        .expect("setcap (libcap2-bin)");
    assert!(st.success(), "setcap");
    older_copy_times_and_marker(&place);
    let before = untouched(&place.copy());
    assert!(before.capability.is_some());

    let (child, status) = place.start(&["capsh", "--drop=cap_net_admin", "--", "-c", r#"exec "$0" "$@""#]);
    stop(child);
    assert_eq!(untouched(&place.copy()), before, "the granted copy was rewritten: its permission is gone");
    assert!(status["core_replaced"].is_null(), "{status}");
    assert_eq!(status["tun_permission"], "granted", "{status}");
}

/// [`older_copy`]'s times and marker on a copy already written.
fn older_copy_times_and_marker(place: &Place) {
    let hour_ago = SystemTime::now() - Duration::from_secs(3600);
    // Opened for writing (to set its time): the copy is run, so not across a fork.
    writing(|| std::fs::File::options().write(true).open(place.copy())?.set_modified(hour_ago)).unwrap();
    set_marker(&place.copy());
}
