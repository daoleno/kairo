//! The connections of one listener, served with limits that hold before
//! anyone signs in: so many connections at once, and a request's head must
//! arrive within a while (a client that sends nothing, or a byte now and
//! then, gives its place up).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ConnectInfo;
use hyper_util::rt::{TokioIo, TokioTimer};
use tower_service::Service;

/// Connections one listener serves at once; more wait to be accepted.
pub const MAX_CONNECTIONS: usize = 512;
/// How long a request's head (its line and headers) may take to arrive.
pub const HEAD_TIME: Duration = Duration::from_secs(20);

/// Serves `app` on what `listener` accepts, until the task is dropped.
pub async fn serve<L>(mut listener: L, app: axum::Router)
where
    L: axum::serve::Listener<Addr = SocketAddr>,
{
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    loop {
        let Ok(slot) = slots.clone().acquire_owned().await else { return };
        let (io, peer) = listener.accept().await;
        let app = app.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let service = hyper::service::service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
                // What `ConnectInfo<SocketAddr>` reads (the sign-in limiter's address).
                request.extensions_mut().insert(ConnectInfo(peer));
                let mut app = app.clone();
                async move { app.call(request).await }
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEAD_TIME)
                .serve_connection(TokioIo::new(io), service)
                // The page's WebSocket.
                .with_upgrades();
            if let Err(e) = connection.await {
                log::debug!("connection from {peer}: {e}");
            }
        });
    }
}
