//! `kairo-core-host`: what Windows runs, as SYSTEM, to give the mihomo core the
//! rights TUN needs (a wintun adapter, routes).
//!
//! ```text
//! kairo-core-host run     --name KairoCore-<user>
//! kairo-core-host service --name KairoCore-<user>
//! ```
//!
//! That is the whole command line, fixed in the task or service when an
//! administrator gave the permission. Everything else (the core, its home
//! directory, the config, the log) is derived from this program's own directory
//! under `%ProgramData%\Kairo`, which only administrators can write to; the host
//! refuses to run from anywhere else. What the user supplies is data, never a
//! path: their config is read from a file only they can write (and not delete or
//! replace), reduced to what the core may run as SYSTEM, and written again by the
//! host; and the core's controller is the host's, answered on the user's port for
//! the few calls Kairo makes. See `host.rs` and the slice 2b note in
//! `RESULT-windows-slice2.md`.

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
