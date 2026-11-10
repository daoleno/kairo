//! The statistics survive a restart, and clearing them clears the file.
//! No core is needed: only the service and its home.

use std::time::Duration;

use kairo_core::Service;
use kairo_core::paths::Paths;
use kairo_core::stats::{Delta, Slot, Store};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_is_recorded_is_kept_across_a_restart_and_clear_history_clears_it() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::new(home.path());
    paths.ensure().unwrap();

    // A file from an earlier run, with this hour's traffic in it.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut store = Store::default();
    store.add_traffic(
        now,
        &[Delta {
            app: "claude".into(),
            slot: Some(Slot::Ai),
            service: Some("claude"),
            bytes: 4242,
        }],
    );
    std::fs::write(paths.stats_file(), store.to_json()).unwrap();

    let svc = Service::start(paths.clone());
    let rx = svc.stats();
    let loaded = loop {
        let s = rx.borrow().clone();
        if !s.is_empty() {
            break s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(loaded.hours.values().map(|h| h.total()).sum::<u64>(), 4242);

    // Clearing empties what the pages read and what is on disk.
    svc.clear_stats();
    assert!(rx.borrow().is_empty());
    let on_disk = Store::from_json(&std::fs::read_to_string(paths.stats_file()).unwrap());
    assert!(on_disk.is_empty());

    // A damaged file is a new, empty one (and does not stop the service).
    svc.shutdown();
    std::fs::write(paths.stats_file(), "{ broken").unwrap();
    let again = Service::start(paths.clone());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(again.stats().borrow().is_empty());
}
