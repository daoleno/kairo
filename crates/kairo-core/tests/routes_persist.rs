//! An app or a site that was added can be removed, and what was removed stays removed after a
//! restart (and what was kept stays). No core is needed: only the service and its home.

use kairo_core::Service;
use kairo_core::paths::Paths;
use kairo_core::routes::{RouteExit, RouteKind};

fn ids(svc: &Service) -> Vec<String> {
    svc.routes().into_iter().map(|r| r.id).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_then_remove_is_gone_after_a_restart() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::new(home.path());
    paths.ensure().unwrap();

    let svc = Service::start(paths.clone());
    svc.set_route(RouteKind::app("telegram"), RouteExit::Direct)
        .await
        .unwrap();
    svc.set_route(RouteKind::site("Example.com"), RouteExit::Block)
        .await
        .unwrap();
    svc.set_route(RouteKind::site("kept.example.org"), RouteExit::Direct)
        .await
        .unwrap();
    assert_eq!(
        ids(&svc),
        ["app:telegram", "site:example.com", "site:kept.example.org"]
    );

    // It is on disk, so a restart has it.
    svc.shutdown();
    let svc = Service::start(paths.clone());
    assert_eq!(
        ids(&svc),
        ["app:telegram", "site:example.com", "site:kept.example.org"]
    );

    // Removing takes the route out of the list at once and out of the file.
    svc.remove_route("app:telegram".into()).await.unwrap();
    svc.remove_route("site:example.com".into()).await.unwrap();
    assert_eq!(ids(&svc), ["site:kept.example.org"]);
    // Removing what is not there is not an error.
    svc.remove_route("site:example.com".into()).await.unwrap();

    svc.shutdown();
    let svc = Service::start(paths.clone());
    assert_eq!(ids(&svc), ["site:kept.example.org"]);

    // "Usual" is the other way to say it, and it also persists.
    svc.set_route(RouteKind::site("kept.example.org"), RouteExit::Usual)
        .await
        .unwrap();
    svc.shutdown();
    let svc = Service::start(paths.clone());
    assert!(ids(&svc).is_empty());
    svc.shutdown();
}
