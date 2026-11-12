//! The local clock. In a browser (`kairo serve`'s page) "local" is the
//! machine Kairo runs on: the statistics are kept in its days, so the page
//! shows them in its time zone too ([`set_remote_offset`]).

#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicI64, Ordering};

#[cfg(target_arch = "wasm32")]
static REMOTE_OFFSET: AtomicI64 = AtomicI64::new(i64::MIN);

/// The time zone of the machine the page shows (its offset from UTC, in
/// seconds), as its server said.
#[cfg(target_arch = "wasm32")]
pub fn set_remote_offset(seconds: i64) {
    REMOTE_OFFSET.store(seconds, Ordering::Relaxed);
}

/// The local wall clock as `HH:MM:SS`.
pub fn clock() -> String {
    let t = (crate::unix() + local_offset_seconds()).rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", t / 3600, t % 3600 / 60, t % 60)
}

/// The local time zone's offset from UTC, in seconds.
pub fn local_offset_seconds() -> i64 {
    local_offset(crate::unix())
}

/// The offset of the local time zone from UTC at `t`, in seconds.
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub fn local_offset(t: i64) -> i64 {
    // SAFETY: localtime_r writes into the struct we give it.
    unsafe {
        let tt: libc::time_t = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&tt, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
}

#[cfg(target_arch = "wasm32")]
pub fn local_offset(t: i64) -> i64 {
    match REMOTE_OFFSET.load(Ordering::Relaxed) {
        i64::MIN => {
            // Before the server said: the browser's own zone.
            let date = js_sys::Date::new(&wasm_bindgen_value(t));
            -(date.get_timezone_offset() as i64) * 60
        }
        offset => offset,
    }
}

#[cfg(target_arch = "wasm32")]
fn wasm_bindgen_value(t: i64) -> js_sys::wasm_bindgen::JsValue {
    js_sys::wasm_bindgen::JsValue::from_f64(t as f64 * 1000.0)
}

#[cfg(not(any(unix, target_arch = "wasm32")))]
pub fn local_offset(_t: i64) -> i64 {
    0
}
