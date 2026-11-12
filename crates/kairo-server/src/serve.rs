//! The connections of one listener, served with limits that hold before
//! anyone signs in: so many connections at once, a request's head must
//! arrive within a while (a client that sends nothing, or a byte now and
//! then, gives its place up), and a client that stops reading what it is
//! sent is let go.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::ConnectInfo;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tower_service::Service;

/// Connections one listener serves at once; more wait to be accepted.
pub const MAX_CONNECTIONS: usize = 512;
/// How long a request's head (its line and headers) may take to arrive.
pub const HEAD_TIME: Duration = Duration::from_secs(20);
/// How long a client may take no byte of what it is sent before its
/// connection ends (and with it the file it was sent, and its places).
pub const STALL_TIME: Duration = Duration::from_secs(60);

/// A connection whose writes fail once the client has taken nothing for
/// [`STALL_TIME`]. Without it, a reader that stops reading holds its
/// connection and its transfer for ever: a write waiting on a full socket
/// is never given up.
struct Stall<I> {
    io: I,
    since: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<I> Stall<I> {
    fn new(io: I) -> Self {
        Stall { io, since: None }
    }

    fn after<T>(&mut self, cx: &mut Context<'_>, polled: Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        if polled.is_ready() {
            self.since = None;
            return polled;
        }
        let since = self.since.get_or_insert_with(|| Box::pin(tokio::time::sleep(STALL_TIME)));
        match since.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, "the client stopped reading"))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<I: AsyncRead + Unpin> AsyncRead for Stall<I> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl<I: AsyncWrite + Unpin> AsyncWrite for Stall<I> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut self.io).poll_write(cx, buf);
        self.after(cx, polled)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut self.io).poll_write_vectored(cx, bufs);
        self.after(cx, polled)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let polled = Pin::new(&mut self.io).poll_flush(cx);
        self.after(cx, polled)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

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
                .serve_connection(TokioIo::new(Stall::new(io)), service)
                // The page's WebSocket.
                .with_upgrades();
            if let Err(e) = connection.await {
                log::debug!("connection from {peer}: {e}");
            }
        });
    }
}
