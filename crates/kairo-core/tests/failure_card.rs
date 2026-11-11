//! The card that says a subscription couldn't update can be closed; it stays closed (also after a
//! restart) until the next update fails. No core is needed: the subscription is at a closed port.

use kairo_core::Service;
use kairo_core::paths::Paths;
use kairo_core::profile::{ConfigStats, Profile, ProfileIndex, Source};

fn shown(svc: &Service) -> bool {
    svc.snapshot().profiles.profiles[0].failure_shown()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_card_returns_with_the_next_failed_update() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::new(home.path());
    paths.ensure().unwrap();
    // A port nothing listens on.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut index = ProfileIndex::default();
    index.profiles.push(Profile {
        id: "a1".into(),
        name: "Backup".into(),
        source: Source::Remote {
            url: format!("http://127.0.0.1:{port}/sub"),
            interval_hours: 0,
        },
        updated_at: Some(1000),
        usage: None,
        web_page: None,
        last_error: None,
        failed_at: None,
        failure_dismissed: false,
        stats: ConfigStats::default(),
    });
    index.save(&paths).unwrap();

    let svc = Service::start(paths.clone());
    assert!(!shown(&svc), "nothing failed yet");

    assert!(svc.update_profile("a1".into()).await.is_err());
    assert!(shown(&svc), "a failed update shows the card");

    // Closing hides it, and the file says so: a restart keeps it closed.
    svc.dismiss_failure("a1");
    assert!(!shown(&svc));
    assert!(svc.snapshot().profiles.profiles[0].last_error.is_some());
    svc.shutdown();
    let svc = Service::start(paths.clone());
    assert!(!shown(&svc), "still closed after a restart");

    // The next failed update says it again, even with the same words.
    assert!(svc.update_profile("a1".into()).await.is_err());
    assert!(shown(&svc), "a new failure shows the card again");
    svc.shutdown();
}
