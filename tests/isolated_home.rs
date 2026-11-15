//! A process that is not Kairo itself, as `nsh test`'s headless runner is (it
//! links this crate and never runs `main`): its home is a scratch directory of
//! its own, it never sets the system proxy or turns TUN on, and the real home is
//! refused before anything is read or started. Nothing here starts a service or
//! a core, or touches the real home: only paths are resolved.

use std::path::PathBuf;

use kairo_core::paths::{HomeEnv, Paths, isolated};

#[test]
fn a_runner_that_never_claims_is_isolated_and_refused_the_real_home() {
    // The app's crate is linked, as the runner links it.
    let _ = nsh_app_kairo::i18n::translations;
    assert!(isolated(), "a process that never claims the real home is isolated");

    let real = HomeEnv::current().real_home();
    // SAFETY: one test in this binary; nothing else reads the environment meanwhile.
    unsafe { std::env::remove_var("KAIRO_HOME") };
    let home = Paths::default_home();
    assert_eq!(home, std::env::temp_dir().join(format!("kairo-isolated-{}", std::process::id())));
    assert_ne!(home, real);

    // The real home, given: a loud stop, before any file is read.
    unsafe { std::env::set_var("KAIRO_HOME", &real) };
    let refused = std::panic::catch_unwind(Paths::default_home).unwrap_err();
    let said = refused.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(said.contains("refusing Kairo's real home"), "{said}");
    unsafe { std::env::remove_var("KAIRO_HOME") };

    // A service started at the real home by hand: stopped before its thread starts.
    let started = std::panic::catch_unwind(|| kairo_core::Service::start(Paths::new(real.clone())));
    assert!(started.is_err(), "Service::start refuses the real home here");

    // No system proxy, whatever the settings say.
    let b = kairo_core::sysproxy::backends();
    assert!(!b.gnome && b.kde.is_none() && !b.mac && !b.windows, "{b:?}");
    assert!(PathBuf::from(&home).starts_with(std::env::temp_dir()));
}
