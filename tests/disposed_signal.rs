//! Page changes with the "goes around Kairo" banner up must not fail a component
//! ("a signal read or written after its owner was disposed", seen once in the Windows
//! log), and what the page raised must not outlive it (a notice from its button).
//!
//! The Windows error itself was not reproduced here (see the commit); this keeps the
//! walk that could, and catches the stuck notice it did find.

use std::sync::Mutex;
use std::time::Duration;

use kairo_core::{Service, bypass::Bypass};
use nsh_app_kairo::App;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

/// What the engine logs and what panics say, kept for the assertion: a failure of a
/// binding or effect is logged, it does not always reach the screen.
static NOTED: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Noting;

impl log::Log for Noting {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Error
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            NOTED.lock().unwrap().push(r.args().to_string());
        }
    }
    fn flush(&self) {}
}

fn failed(app: &Headless) -> Vec<String> {
    app.screen()
        .texts()
        .into_iter()
        .filter(|t| t.contains("failed:") || t.contains("owner was disposed"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn leaving_a_page_with_the_bypass_banner_up_fails_nothing() {
    let home = std::env::temp_dir().join(format!("kairo-disposed-{}", std::process::id()));
    // SAFETY: set before anything reads them, in a test binary with one test.
    unsafe {
        std::env::set_var("KAIRO_HOME", &home);
        std::env::set_var("KAIRO_NO_BYPASS", "1");
    }
    let _ = log::set_logger(&Noting);
    log::set_max_level(log::LevelFilter::Error);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        NOTED.lock().unwrap().push(info.to_string());
        default_hook(info);
    }));
    let svc = Service::global();
    let tool = |name: &str, pid: u32| Bypass {
        tool: name.into(),
        service: (name == "claude").then_some("claude"),
        pid,
        remote: "93.184.216.34:443".into(),
        since: 0,
    };
    let mut app = Headless::start(EngineConfig::new(Root::of::<App>())).unwrap();
    app.first_render().unwrap();
    // A desktop window: the default surface is too small for the banner's row.
    app.set_viewport(nsh_core::prelude::size(
        nsh_core::prelude::px(1100.0),
        nsh_core::prelude::px(760.0),
    ))
    .unwrap();
    app.press("text:Activity").unwrap();
    svc.inject_bypass(vec![tool("claude", 11)]);
    app.wait(Duration::from_millis(800)).unwrap();
    app.wait_for("text:claude goes around Kairo").unwrap();

    // The list changes while the banner is up, and the page is left in the middle of it.
    // A tool of the banner, pressed, then the page is left with its notice still up.
    app.press("text:Copy for terminal").unwrap();
    app.press("text:Exits").unwrap();
    app.wait(Duration::from_secs(4)).unwrap();
    assert!(
        !app.screen()
            .texts()
            .iter()
            .any(|t| t.starts_with("Copied.")),
        "the notice stays up after its page is left"
    );
    app.press("text:Activity").unwrap();
    log::error!("probe");
    assert_eq!(NOTED.lock().unwrap().len(), 1, "the log capture works");
    NOTED.lock().unwrap().clear();
    for round in 0..6 {
        app.press("text:All Connections").unwrap();
        svc.inject_bypass(vec![tool("claude", 11), tool("cursor", 13 + round)]);
        app.press("text:By App").unwrap();
        svc.inject_bypass(vec![tool("claude", 11), tool("cursor", 12 + round)]);
        app.press("text:Exits").unwrap();
        svc.inject_bypass(vec![]);
        app.press("text:Activity").unwrap();
        svc.inject_bypass(vec![tool("claude", 11)]);
        app.wait(Duration::from_millis(120)).unwrap();
        app.press("text:Home").unwrap();
        app.wait(Duration::from_millis(60)).unwrap();
        svc.inject_bypass(vec![]);
        app.press("text:Apps & Sites").unwrap();
        svc.inject_bypass(vec![tool("claude", 11)]);
        app.wait(Duration::from_millis(60)).unwrap();
        app.press("text:Activity").unwrap();
    }
    app.wait(Duration::from_millis(500)).unwrap();
    let mut bad = failed(&app);
    bad.extend(NOTED.lock().unwrap().iter().cloned());
    assert!(bad.is_empty(), "{bad:#?}");
    let _ = std::fs::remove_dir_all(home);
}
