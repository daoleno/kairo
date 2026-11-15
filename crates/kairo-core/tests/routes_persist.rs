//! An app of the person's that was added can be removed, and what was removed stays removed
//! after a restart (and what was kept stays); a catalog app's own exit persists too, and back
//! on its kind it leaves the list. No core is needed: only the service and its home.

use kairo_core::Service;
use kairo_core::kinds::{AppExit, Kind};
use kairo_core::paths::Paths;
use kairo_core::routes::RouteKind;

fn ids(svc: &Service) -> Vec<String> {
    svc.apps().into_iter().map(|r| r.id).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_then_remove_is_gone_after_a_restart() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::new(home.path());
    paths.ensure().unwrap();

    let svc = Service::start(paths.clone());
    let own = |m: RouteKind| (m.id(), Some(m));
    let (id, m) = own(RouteKind::app("telegram"));
    svc.set_app(id, m, Kind::Other, AppExit::Direct).await.unwrap();
    let (id, m) = own(RouteKind::site("Example.com"));
    svc.set_app(id, m, Kind::Other, AppExit::Block).await.unwrap();
    let (id, m) = own(RouteKind::site("kept.example.org"));
    svc.set_app(id, m, Kind::Other, AppExit::Direct).await.unwrap();
    svc.set_app("netflix".into(), None, Kind::Streaming, AppExit::Direct).await.unwrap();
    assert_eq!(
        ids(&svc),
        ["app:telegram", "site:example.com", "site:kept.example.org", "netflix"]
    );
    // A browser is never an app.
    assert!(svc.set_app("app:firefox".into(), Some(RouteKind::app("firefox")), Kind::Other, AppExit::Direct).await.is_err());

    // It is on disk, so a restart has it.
    svc.shutdown();
    let svc = Service::start(paths.clone());
    assert_eq!(
        ids(&svc),
        ["app:telegram", "site:example.com", "site:kept.example.org", "netflix"]
    );

    // Removing takes the app out of the list at once and out of the file.
    svc.remove_app("app:telegram".into()).await.unwrap();
    svc.remove_app("site:example.com".into()).await.unwrap();
    assert_eq!(ids(&svc), ["site:kept.example.org", "netflix"]);
    // Removing what is not there is not an error.
    svc.remove_app("site:example.com".into()).await.unwrap();

    svc.shutdown();
    let svc = Service::start(paths.clone());
    assert_eq!(ids(&svc), ["site:kept.example.org", "netflix"]);

    // A catalog app back on its kind leaves the list, and that persists.
    svc.set_app("netflix".into(), None, Kind::Streaming, AppExit::Kind).await.unwrap();
    svc.shutdown();
    let svc = Service::start(paths.clone());
    assert_eq!(ids(&svc), ["site:kept.example.org"]);
    svc.shutdown();
}
