//! Kairo on Android: the app is also a `VpnService`, and the mihomo core runs as
//! a child process of the app with the VPN's tun device handed to it.
//!
//! The Java side (`android/java/dev/kairo/app`) owns everything Android makes
//! only Java able to do: the VPN consent screen, `VpnService.Builder`, the
//! foreground notification. This module is the Rust end of that small bridge:
//!
//! - Java calls in with ordinary JNI-named functions (`Java_dev_kairo_app_Bridge_*`,
//!   found by name, so there is no `RegisterNatives`): at start-up with the
//!   app's directories, and when the user turns the VPN off in system settings.
//! - Rust calls out through [`establish`] / [`teardown`], which run on a thread
//!   attached to the JVM. `establish` blocks until the VPN is up (it may show
//!   the consent screen) and returns a duplicate of the tun file descriptor,
//!   which a child process inherits: it is left without `FD_CLOEXEC`.
//!
//! The core binary is `libmihomo.so` in the app's native library directory (the
//! one place an app may execute a file from; the APK holds it with
//! `extractNativeLibs`).

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};

use jni::objects::{GlobalRef, JClass, JString};
use jni::{JNIEnv, JavaVM};

struct Bridge {
    vm: JavaVM,
    class: GlobalRef,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();
static FILES: OnceLock<PathBuf> = OnceLock::new();
static NATIVE: OnceLock<PathBuf> = OnceLock::new();
/// The tun descriptor the running core was started with, -1 when none.
static TUN_FD: AtomicI32 = AtomicI32::new(-1);

fn string(env: &mut JNIEnv<'_>, s: &JString<'_>) -> String {
    env.get_string(s).map(Into::into).unwrap_or_default()
}

/// `Bridge.nativeInit(String filesDir, String nativeLibraryDir)`, from the
/// application's `onCreate`: the only thread where the app's classes can be found.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_kairo_app_Bridge_nativeInit(
    mut env: JNIEnv<'_>,
    class: JClass<'_>,
    files: JString<'_>,
    native_dir: JString<'_>,
) {
    let _ = FILES.set(PathBuf::from(string(&mut env, &files)));
    let _ = NATIVE.set(PathBuf::from(string(&mut env, &native_dir)));
    let (Ok(vm), Ok(class)) = (env.get_java_vm(), env.new_global_ref(class)) else {
        return;
    };
    let _ = BRIDGE.set(Bridge { vm, class });
}

/// `Bridge.nativeRevoked()`: the user (or another VPN app) took the VPN away.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_kairo_app_Bridge_nativeRevoked(_env: JNIEnv<'_>, _class: JClass<'_>) {
    if let Some(svc) = crate::Service::try_global() {
        // `connect` starts its work on the service's runtime when called.
        drop(svc.connect(false));
    }
}

/// The app's private files directory (the home of Kairo's files).
pub fn files_dir() -> Option<PathBuf> {
    FILES.get().cloned()
}

/// The core: `libmihomo.so` in the native library directory.
pub fn core() -> Option<PathBuf> {
    NATIVE.get().map(|d| d.join("libmihomo.so"))
}

/// The tun descriptor for the core's config, when the VPN is up.
pub fn tun_fd() -> Option<i32> {
    let fd = TUN_FD.load(Ordering::SeqCst);
    (fd >= 0).then_some(fd)
}

/// Brings the VPN up and returns a descriptor of its tun device for the core
/// (a duplicate, owned by the caller until [`teardown`]). Blocks: run it on a
/// blocking thread.
pub fn establish() -> Result<i32, String> {
    let bridge = BRIDGE
        .get()
        .ok_or("Kairo's Android bridge is not set up")?;
    let mut env = bridge
        .vm
        .attach_current_thread()
        .map_err(|e| format!("JNI: {e}"))?;
    let class: &JClass<'_> = bridge.class.as_obj().into();
    let fd = env
        .call_static_method(class, "establish", "()I", &[])
        .and_then(|v| v.i())
        .map_err(|e| {
            let _ = env.exception_clear();
            format!("JNI: {e}")
        })?;
    match fd {
        -1 => Err("The VPN permission was not given".into()),
        -2 => Err("Android could not set up the VPN".into()),
        fd if fd < 0 => Err(format!("The VPN could not start ({fd})")),
        fd => {
            // A duplicate: it has no FD_CLOEXEC, so the core inherits it, and
            // it stays open when the service closes its own copy.
            // SAFETY: duplicating a descriptor the service just gave us.
            let own = unsafe { libc::dup(fd) };
            if own < 0 {
                return Err("The VPN's descriptor could not be duplicated".into());
            }
            let old = TUN_FD.swap(own, Ordering::SeqCst);
            if old >= 0 {
                // SAFETY: the previous duplicate, no longer used.
                unsafe { libc::close(old) };
            }
            Ok(own)
        }
    }
}

/// Takes the VPN down and closes our copy of its descriptor.
pub fn teardown() {
    let fd = TUN_FD.swap(-1, Ordering::SeqCst);
    if fd >= 0 {
        // SAFETY: our duplicate from `establish`.
        unsafe { libc::close(fd) };
    }
    let Some(bridge) = BRIDGE.get() else { return };
    let class: &JClass<'_> = bridge.class.as_obj().into();
    if let Ok(mut env) = bridge.vm.attach_current_thread()
        && env.call_static_method(class, "teardown", "()V", &[]).is_err()
    {
        let _ = env.exception_clear();
    }
}
