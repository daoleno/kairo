//! `kairo-server` is Kairo itself: with no `KAIRO_HOME` it uses the real home
//! (the platform's data folder under HOME), never the scratch home that any
//! other process gets (`kairo_core::paths::isolated`). If its `main` stopped
//! claiming the real home, people's settings and subscriptions would seem gone.
//! HOME is a temporary folder here, so the real home is one too.

#![cfg(unix)]

use std::process::Command;

#[test]
fn kairo_server_uses_the_real_home_not_one_of_its_own() {
    let home = tempfile::tempdir().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_kairo-server"))
        .args(["status", "--json"])
        .env_remove("KAIRO_HOME")
        .env_remove("XDG_DATA_HOME")
        .env("HOME", home.path())
        .env("TMPDIR", temp.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let expected = if cfg!(target_os = "macos") {
        home.path().join("Library/Application Support/Kairo")
    } else {
        home.path().join(".local/share/kairo")
    };
    assert_eq!(status["home"], expected.to_string_lossy().as_ref(), "{status}");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0, "no scratch home was made");
}
