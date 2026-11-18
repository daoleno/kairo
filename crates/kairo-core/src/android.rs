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
//! The engine starts with the process (`Bridge.nativeStart`, from the
//! application's `onCreate`), not with the window: the VPN service, Always-on
//! VPN, the boot and update receivers and the Quick Settings tile connect
//! without a screen (`nativeConnect`, `nativeRestore`), and the tile follows
//! the engine's state (`Bridge.stateChanged`).
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
/// The device's language (a BCP 47 tag), for Kairo's "System" language.
static LOCALE: OnceLock<String> = OnceLock::new();
/// The engine finished starting (see [`booted`]).
static BOOTED: std::sync::LazyLock<tokio::sync::watch::Sender<bool>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::Sender::new(false));
/// The tun descriptor the running core was started with, -1 when none.
static TUN_FD: AtomicI32 = AtomicI32::new(-1);

fn string(env: &mut JNIEnv<'_>, s: &JString<'_>) -> String {
    env.get_string(s).map(Into::into).unwrap_or_default()
}

/// `Bridge.nativeInit(String filesDir, String nativeLibraryDir, String locale)`, from
/// the application's `onCreate`: the only thread where the app's classes can be found.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_kairo_app_Bridge_nativeInit(
    mut env: JNIEnv<'_>,
    class: JClass<'_>,
    files: JString<'_>,
    native_dir: JString<'_>,
    locale: JString<'_>,
) {
    let _ = FILES.set(PathBuf::from(string(&mut env, &files)));
    let _ = NATIVE.set(PathBuf::from(string(&mut env, &native_dir)));
    let _ = LOCALE.set(string(&mut env, &locale));
    let (Ok(vm), Ok(class)) = (env.get_java_vm(), env.new_global_ref(class)) else {
        return;
    };
    let _ = BRIDGE.set(Bridge { vm, class });
}

/// `Bridge.nativeStart()`, from the application's `onCreate` after `nativeInit`:
/// the engine, in the language picked in Settings (System: the device's), and
/// the tile following its state. It does not connect by itself.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_kairo_app_Bridge_nativeStart(_env: JNIEnv<'_>, _class: JClass<'_>) {
    let svc = crate::Service::global();
    let tag = svc
        .settings()
        .language
        .tag()
        .map(str::to_owned)
        .or_else(|| LOCALE.get().cloned().filter(|l| !l.is_empty()))
        .unwrap_or_else(|| "en".into());
    crate::i18n::set_language(&tag);
    let watched = svc.clone();
    svc.runtime().spawn(async move { follow_state(watched).await });
}

/// `Bridge.nativeConnect(boolean)`: the tile, Always-on VPN (on), the tile (off).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_kairo_app_Bridge_nativeConnect(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    on: jni::sys::jboolean,
) {
    connect(on != 0);
}

/// `Bridge.nativeWantsOn()`: Kairo was on, with a subscription to be on with.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_kairo_app_Bridge_nativeWantsOn(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jni::sys::jboolean {
    crate::Service::try_global().is_some_and(|svc| wants_on(&svc)).into()
}

/// `Bridge.nativeRestore()`: on again if Kairo was on (the app opened, the
/// service restarted after Android killed it, the phone started, an update).
/// False when it stays off.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_kairo_app_Bridge_nativeRestore(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jni::sys::jboolean {
    let on = crate::Service::try_global().is_some_and(|svc| wants_on(&svc));
    if on {
        connect(true);
    }
    on.into()
}

/// Whether Kairo should be on: it was left on, and has a subscription (the VPN,
/// and its permission screen, wait for one).
fn wants_on(svc: &crate::Service) -> bool {
    svc.settings().auto_start_core && svc.state().borrow().profiles.active.is_some()
}

/// Turns Kairo on or off once the engine has started. A failed start takes the
/// VPN service down with it: it may be in the foreground with no tun yet.
fn connect(on: bool) {
    let Some(svc) = crate::Service::try_global() else {
        return;
    };
    svc.runtime().spawn(async move {
        let mut booted = BOOTED.subscribe();
        let _ = booted.wait_for(|b| *b).await;
        if let Err(e) = svc.connect(on).await {
            log::warn!("connect: {e}");
            if on {
                let _ = tokio::task::spawn_blocking(teardown).await;
            }
        }
    });
}

/// The engine has started (`Service::boot`): connecting may begin.
pub(crate) fn booted() {
    BOOTED.send_replace(true);
}

/// The tile's states (`Bridge.OFF` …).
const TILE_OFF: i32 = 0;
const TILE_CONNECTING: i32 = 1;
const TILE_ON: i32 = 2;
const TILE_UNAVAILABLE: i32 = 3;

/// What the tile shows for `state`.
fn tile_state(state: &crate::State) -> i32 {
    use crate::CoreStatus;
    if state.profiles.active.is_none() {
        return TILE_UNAVAILABLE;
    }
    match state.core {
        CoreStatus::Running { .. } => TILE_ON,
        CoreStatus::Starting => TILE_CONNECTING,
        CoreStatus::Stopped | CoreStatus::Stopping | CoreStatus::Failed { .. } => TILE_OFF,
    }
}

/// Tells the tile the engine's state, and its words, whenever either changes.
async fn follow_state(svc: crate::Service) {
    let mut state = svc.state();
    let mut language = crate::i18n::watch();
    let mut shown: Option<(i32, String)> = None;
    loop {
        let now = (tile_state(&state.borrow_and_update()), language.borrow_and_update().clone());
        if shown.as_ref() != Some(&now) {
            let code = now.0;
            let _ = tokio::task::spawn_blocking(move || tell_tile(code)).await;
            shown = Some(now);
        }
        tokio::select! {
            r = state.changed() => if r.is_err() { return },
            r = language.changed() => if r.is_err() { return },
        }
    }
}

/// `Bridge.stateChanged(int, String[])`: the state and the subtitles in the language shown.
fn tell_tile(code: i32) {
    let Some(bridge) = BRIDGE.get() else { return };
    let Ok(mut env) = bridge.vm.attach_current_thread() else {
        return;
    };
    let texts = [
        crate::tr!("engine.android.tile_off"),
        crate::tr!("tray.status.connecting"),
        crate::tr!("engine.android.tile_on"),
        crate::tr!("engine.android.tile_unavailable"),
    ];
    let class: &JClass<'_> = bridge.class.as_obj().into();
    let made = (|| -> jni::errors::Result<()> {
        let array = env.new_object_array(texts.len() as i32, "java/lang/String", jni::objects::JObject::null())?;
        for (i, t) in texts.iter().enumerate() {
            let s = env.new_string(t)?;
            env.set_object_array_element(&array, i as i32, s)?;
        }
        env.call_static_method(class, "stateChanged", "(I[Ljava/lang/String;)V", &[code.into(), (&array).into()])?;
        Ok(())
    })();
    if made.is_err() {
        let _ = env.exception_clear();
    }
}

/// Whether Settings offers to add Kairo's tile to Quick Settings (Android 13+,
/// not added yet).
pub fn tile_offered() -> bool {
    let Some(bridge) = BRIDGE.get() else { return false };
    let Ok(mut env) = bridge.vm.attach_current_thread() else {
        return false;
    };
    let class: &JClass<'_> = bridge.class.as_obj().into();
    match env.call_static_method(class, "tileOffered", "()Z", &[]).and_then(|v| v.z()) {
        Ok(b) => b,
        Err(_) => {
            let _ = env.exception_clear();
            false
        }
    }
}

/// Android's own "Add tile" request (Settings' Add).
pub fn request_tile() {
    let Some(bridge) = BRIDGE.get() else { return };
    let Ok(mut env) = bridge.vm.attach_current_thread() else {
        return;
    };
    let class: &JClass<'_> = bridge.class.as_obj().into();
    if env.call_static_method(class, "requestTile", "()V", &[]).is_err() {
        let _ = env.exception_clear();
    }
}

/// `Bridge.nativeRevoked()`: the user (or another VPN app) took the VPN away.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_kairo_app_Bridge_nativeRevoked(_env: JNIEnv<'_>, _class: JClass<'_>) {
    if let Some(svc) = crate::Service::try_global() {
        // `connect` starts its work on the service's runtime when called.
        drop(svc.connect(false));
    }
}

/// `Bridge.nativeDisconnect()`: the notification's Disconnect button.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_kairo_app_Bridge_nativeDisconnect(_env: JNIEnv<'_>, _class: JClass<'_>) {
    if let Some(svc) = crate::Service::try_global() {
        drop(svc.connect(false));
    }
}

/// `Bridge.nativeInstallResult(int, String)`: the package installer refused the
/// update (or the person cancelled it).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_kairo_app_Bridge_nativeInstallResult(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    status: jni::sys::jint,
    message: JString<'_>,
) {
    let message = string(&mut env, &message);
    if let Some(svc) = crate::Service::try_global() {
        svc.install_result(status, &message);
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
        .ok_or_else(|| crate::tr!("engine.android.no_bridge"))?;
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
        -1 => Err(crate::tr!("engine.android.vpn_denied")),
        -2 => Err(crate::tr!("engine.android.vpn_failed")),
        -3 => Err(crate::tr!("engine.android.vpn_needs_app")),
        fd if fd < 0 => Err(crate::tr!("engine.android.vpn_error", code = fd)),
        fd => {
            // A duplicate: it has no FD_CLOEXEC, so the core inherits it, and
            // it stays open when the service closes its own copy.
            // SAFETY: duplicating a descriptor the service just gave us.
            let own = unsafe { libc::dup(fd) };
            if own < 0 {
                return Err(crate::tr!("engine.android.dup_failed"));
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

/// Gives the APK at `path` to Android's package installer (a session the user
/// confirms). Returns once the session is committed.
pub fn install_apk(path: &std::path::Path) -> Result<(), String> {
    let bridge = BRIDGE
        .get()
        .ok_or_else(|| crate::tr!("engine.android.no_bridge"))?;
    let mut env = bridge
        .vm
        .attach_current_thread()
        .map_err(|e| format!("JNI: {e}"))?;
    let class: &JClass<'_> = bridge.class.as_obj().into();
    let path = env
        .new_string(path.to_string_lossy().as_ref())
        .map_err(|e| format!("JNI: {e}"))?;
    let result = env
        .call_static_method(
            class,
            "installApk",
            "(Ljava/lang/String;)Ljava/lang/String;",
            &[(&path).into()],
        )
        .and_then(|v| v.l());
    match result {
        Ok(msg) if msg.is_null() => Ok(()),
        Ok(msg) => Err(env
            .get_string(&JString::from(msg))
            .map(Into::into)
            .unwrap_or_else(|_| crate::tr!("engine.android.installer_failed"))),
        Err(e) => {
            let _ = env.exception_clear();
            Err(format!("JNI: {e}"))
        }
    }
}

/// The icon of the installed app called `name` (a package, or its label) as PNG bytes.
pub fn app_icon(name: &str) -> Option<Vec<u8>> {
    let bridge = BRIDGE.get()?;
    let mut env = bridge.vm.attach_current_thread().ok()?;
    let class: &JClass<'_> = bridge.class.as_obj().into();
    let name = env.new_string(name).ok()?;
    let result = env
        .call_static_method(class, "appIcon", "(Ljava/lang/String;)[B", &[(&name).into()])
        .and_then(|v| v.l());
    match result {
        Ok(bytes) if !bytes.is_null() => {
            let array = jni::objects::JByteArray::from(bytes);
            env.convert_byte_array(&array).ok()
        }
        Ok(_) => None,
        Err(_) => {
            let _ = env.exception_clear();
            None
        }
    }
}

/// The name the installed app called `name` (a package, or its label) shows people.
pub fn app_label(name: &str) -> Option<String> {
    let bridge = BRIDGE.get()?;
    let mut env = bridge.vm.attach_current_thread().ok()?;
    let class: &JClass<'_> = bridge.class.as_obj().into();
    let name = env.new_string(name).ok()?;
    let result = env
        .call_static_method(
            class,
            "appLabel",
            "(Ljava/lang/String;)Ljava/lang/String;",
            &[(&name).into()],
        )
        .and_then(|v| v.l());
    match result {
        Ok(label) if !label.is_null() => {
            let label = jni::objects::JString::from(label);
            let text = env.get_string(&label).ok().map(String::from);
            text
        }
        Ok(_) => None,
        Err(_) => {
            let _ = env.exception_clear();
            None
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
