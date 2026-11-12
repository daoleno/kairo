//! Kairo in a browser: the page of `kairo-server`. Before the pages, the
//! door: the first password (with the code the server printed), or signing
//! in. Then one WebSocket carries everything ([`crate::backend::Remote`]);
//! it comes back by itself after a drop, and a session the server ended
//! returns to the door.
//!
//! The session is an HttpOnly cookie the server sets: nothing here reads,
//! keeps or sends a secret of its own, and no address carries one.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use nsh_base::theme;
use nsh_core::AutoFill;
use nsh_core::platform::{HttpBody, HttpRequest};
use nsh_core::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::backend::{Backend, Remote};
use crate::ui::{self, Variant, size};

/// Kairo's icon.
const ICON: &str = include_str!("../assets/icon.svg");

/// Where the person is.
#[derive(Clone, PartialEq)]
enum Door {
    Asking,
    /// No password yet: one is set with the server's one-time code.
    Setup,
    SignIn,
    In,
    /// The server could not be reached.
    Away(String),
}

/// What `/api/session` says.
#[derive(Deserialize)]
struct Session {
    /// `setup`, `sign_in` or `signed_in`.
    state: String,
    #[serde(default)]
    machine: String,
}

/// The page's root.
pub fn app() -> impl IntoElement {
    let door = signal(Door::Asking);
    let machine = signal(String::new());
    let ask = move || {
        spawn(async move {
            match get_json::<Session>("/api/session").await {
                Ok(s) => {
                    machine.set(s.machine);
                    door.set(match s.state.as_str() {
                        "setup" => Door::Setup,
                        "signed_in" => Door::In,
                        _ => Door::SignIn,
                    });
                }
                Err(e) => door.set(Door::Away(e)),
            }
        });
    };
    ask();
    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(dynamic(move || match door.get() {
            Door::Asking => view().flex_1().into_element(),
            Door::Setup => setup(machine, door).into_element(),
            Door::SignIn => sign_in(machine, door).into_element(),
            Door::Away(why) => away(why, ask).into_element(),
            Door::In => untracked(move || connected(door)).into_element(),
        }))
}

/// The pages, over a link to the server.
fn connected(door: Signal<Door>) -> AnyElement {
    let remote = Remote::new();
    let ready = signal(false);
    let link = Link::open(remote.clone());
    let online = signal(false);
    {
        let mut on = remote.online();
        spawn(async move {
            loop {
                online.set(*on.borrow_and_update());
                if on.changed().await.is_err() {
                    break;
                }
            }
        });
    }
    {
        let r = remote.clone();
        let mut out = r.signed_out();
        spawn(async move {
            while out.changed().await.is_ok() {
                if *out.borrow() {
                    door.set(Door::SignIn);
                    break;
                }
            }
        });
    }
    {
        // The pages start once the server has sent everything once.
        let mut synced = remote.synced();
        spawn(async move {
            while !*synced.borrow_and_update() {
                if synced.changed().await.is_err() {
                    return;
                }
            }
            ready.set(true);
        });
    }
    // The link ends with this view (signed out, or the page left).
    cleanup(move || link.close());
    view()
        .flex_1()
        .child(dynamic(move || {
            if ready.get() {
                untracked(|| crate::main_view(Backend::Remote(remote.clone()))).into_element()
            } else {
                view().flex_1().into_element()
            }
        }))
        .child(dynamic(move || {
            if ready.get() && !online.get() {
                lost_banner().into_element()
            } else {
                view().into_element()
            }
        }))
        .into_element()
}

/// A thin line over the window while the link to the server is down.
fn lost_banner() -> impl IntoElement {
    view()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .py_1()
        .items_center()
        .bg(move || theme().muted)
        .child(
            text("Reconnecting…")
                .text_size(px(size::small()))
                .text_color(move || theme().muted_foreground),
        )
}

// --- The door ----------------------------------------------------------------------------

fn panel(title: impl Into<SharedString>, machine: Signal<String>) -> View {
    let title: SharedString = title.into();
    view()
        .w_full()
        .max_w(px(360.0))
        .gap_3()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(svg(nsh_core::element::ImageSrc::svg_markup(ICON)).size(px(28.0)))
                .child(ui::title(title)),
        )
        .child(
            text(move || machine.get())
                .text_size(px(size::small()))
                .text_color(move || theme().muted_foreground),
        )
}

fn centered(child: impl IntoElement) -> View {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .px_4()
        .child(child)
}

fn password_field(value: Signal<String>, placeholder: &'static str, new: bool) -> nsh_base::Input {
    ui::field(value, placeholder)
        .masked(true)
        .auto_fill(if new { AutoFill::NewPassword } else { AutoFill::Password })
        .h(px(36.0))
}

fn problem_line(problem: Signal<Option<String>>) -> impl IntoElement {
    dynamic(move || match problem.get() {
        Some(p) => text(p)
            .text_size(px(size::small()))
            .text_color(move || theme().destructive)
            .into_element(),
        None => view().into_element(),
    })
}

fn setup(machine: Signal<String>, door: Signal<Door>) -> impl IntoElement {
    let code = signal(String::new());
    let password = signal(String::new());
    let again = signal(String::new());
    let problem = signal(None::<String>);
    let busy = signal(false);
    let submit = move || {
        if busy.get_untracked() {
            return;
        }
        let (c, p, a) = (code.get_untracked(), password.get_untracked(), again.get_untracked());
        if p.chars().count() < 8 {
            problem.set(Some("Use at least 8 characters".into()));
            return;
        }
        if p != a {
            problem.set(Some("The passwords differ".into()));
            return;
        }
        busy.set(true);
        spawn(async move {
            let body = serde_json::json!({ "code": c.trim(), "password": p });
            let r = post_json("/api/setup", &body).await;
            // The door changes last: this view (and its signals) goes with it.
            busy.set(false);
            match r {
                Ok(_) => door.set(Door::In),
                Err(e) => problem.set(Some(e)),
            }
        });
    };
    centered(
        panel("Set a password", machine)
            .child(
                text("Enter the code Kairo printed when it started on this machine (kairo-server status shows it).")
                    .text_size(px(size::small()))
                    .text_color(move || theme().muted_foreground),
            )
            .child(ui::field(code, "Setup code").h(px(36.0)).auto_focus(true))
            .child(password_field(password, "New password", true))
            .child(password_field(again, "New password again", true).on_submit(move |_: &str| submit()))
            .child(problem_line(problem))
            .child(
                ui::btn(move || -> SharedString { if busy.get() { "Setting…".into() } else { "Set Password".into() } }, Variant::Primary)
                    .disabled(move || busy.get())
                    .on_press(submit),
            ),
    )
}

fn sign_in(machine: Signal<String>, door: Signal<Door>) -> impl IntoElement {
    let password = signal(String::new());
    let problem = signal(None::<String>);
    let busy = signal(false);
    let submit = move || {
        if busy.get_untracked() || password.get_untracked().is_empty() {
            return;
        }
        busy.set(true);
        let p = password.get_untracked();
        spawn(async move {
            let r = post_json("/api/login", &serde_json::json!({ "password": p })).await;
            // The door changes last: this view (and its signals) goes with it.
            busy.set(false);
            match r {
                Ok(_) => door.set(Door::In),
                Err(e) => {
                    problem.set(Some(e));
                    password.set(String::new());
                }
            }
        });
    };
    centered(
        panel("Sign in to Kairo", machine)
            .child(password_field(password, "Password", false).auto_focus(true).on_submit(move |_: &str| submit()))
            .child(problem_line(problem))
            .child(
                ui::btn(move || -> SharedString { if busy.get() { "Signing in…".into() } else { "Sign In".into() } }, Variant::Primary)
                    .disabled(move || busy.get())
                    .on_press(submit),
            ),
    )
}

fn away(why: String, retry: impl Fn() + Copy + 'static) -> impl IntoElement {
    centered(
        view()
            .gap_3()
            .items_center()
            .child(ui::title("Kairo is not answering"))
            .child(
                text(why)
                    .text_size(px(size::small()))
                    .text_color(move || theme().muted_foreground),
            )
            .child(ui::btn("Try Again", Variant::Secondary).on_press(retry)),
    )
}

// --- HTTP --------------------------------------------------------------------------------

fn absolute(path: &str) -> String {
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    format!("{origin}{path}")
}

/// What the server's error answers carry.
#[derive(Deserialize)]
struct Problem {
    error: String,
}

async fn send(request: HttpRequest) -> Result<String, String> {
    let response = fetch(request).await.map_err(|_| "Kairo is not answering".to_owned())?;
    let text = response.text();
    if response.ok() {
        Ok(text)
    } else {
        Err(serde_json::from_str::<Problem>(&text)
            .map(|p| p.error)
            .unwrap_or_else(|_| format!("The server answered {}", response.status())))
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, String> {
    let text = send(HttpRequest::get(absolute(path))).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Signs this browser out: the server ends the session and clears the
/// cookie; the link closes and the page asks to sign in.
pub async fn sign_out() -> Result<(), String> {
    post_json("/api/logout", &serde_json::json!({})).await.map(|_| ())
}

async fn post_json(path: &str, body: &serde_json::Value) -> Result<String, String> {
    let mut request = HttpRequest::new("POST", absolute(path));
    request.headers.push(("Content-Type".into(), "application/json".into()));
    request.body = HttpBody::Bytes(body.to_string().into_bytes());
    send(request).await
}

// --- The link ----------------------------------------------------------------------------

/// The WebSocket to the server, opened again after a drop.
struct Link {
    inner: Rc<RefCell<LinkInner>>,
}

struct LinkInner {
    socket: Option<web_sys::WebSocket>,
    closed: bool,
}

impl Clone for Link {
    fn clone(&self) -> Self {
        Link { inner: self.inner.clone() }
    }
}

/// Close codes of the server: the session is no more.
const SIGNED_OUT: u16 = 4401;

impl Link {
    fn open(remote: Remote) -> Link {
        let link = Link {
            inner: Rc::new(RefCell::new(LinkInner { socket: None, closed: false })),
        };
        // Browser events reach the engine through this channel (signals
        // are touched on the engine's thread only).
        let (tx, mut rx) = kairo_core::tokio::sync::mpsc::unbounded_channel::<Event>();
        let l = link.clone();
        let r = remote.clone();
        spawn(async move {
            let mut wait = 1u64;
            l.connect(tx.clone());
            while let Some(event) = rx.recv().await {
                match event {
                    Event::Open(socket) => {
                        wait = 1;
                        r.set_link(Some(Box::new(move |text: String| {
                            let _ = socket.send_with_str(&text);
                        })));
                    }
                    Event::Message(text) => {
                        if let Err(e) = r.receive(&text) {
                            log::warn!("a message Kairo did not read: {e}");
                        }
                    }
                    Event::Closed(code) => {
                        r.set_link(None);
                        if l.inner.borrow().closed {
                            break;
                        }
                        if code == SIGNED_OUT {
                            r.mark_signed_out();
                            break;
                        }
                        timer(Duration::from_secs(wait)).await;
                        wait = (wait * 2).min(15);
                        if l.inner.borrow().closed {
                            break;
                        }
                        l.connect(tx.clone());
                    }
                }
            }
        });
        link
    }

    fn connect(&self, tx: kairo_core::tokio::sync::mpsc::UnboundedSender<Event>) {
        let Some(window) = web_sys::window() else { return };
        let location = window.location();
        let scheme = if location.protocol().ok().as_deref() == Some("https:") { "wss" } else { "ws" };
        let url = format!("{scheme}://{}/api/ws", location.host().unwrap_or_default());
        let socket = match web_sys::WebSocket::new(&url) {
            Ok(s) => s,
            Err(_) => {
                let _ = tx.send(Event::Closed(1006));
                return;
            }
        };
        let t = tx.clone();
        let s = socket.clone();
        let onopen = Closure::<dyn FnMut()>::new(move || {
            let _ = t.send(Event::Open(s.clone()));
        });
        let t = tx.clone();
        let onmessage = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            if let Some(text) = e.data().as_string() {
                let _ = t.send(Event::Message(text));
            }
        });
        let t = tx;
        let onclose = Closure::<dyn FnMut(web_sys::CloseEvent)>::new(move |e: web_sys::CloseEvent| {
            let _ = t.send(Event::Closed(e.code()));
        });
        socket.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        socket.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        // They live as long as the socket does.
        onopen.forget();
        onmessage.forget();
        onclose.forget();
        self.inner.borrow_mut().socket = Some(socket);
    }

    fn close(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.closed = true;
        if let Some(s) = inner.socket.take() {
            let _ = s.close();
        }
    }
}

enum Event {
    Open(web_sys::WebSocket),
    Message(String),
    Closed(u16),
}

// --- Files -------------------------------------------------------------------------------

/// Asks the person for a text file on their device: its name and text, or
/// nothing when they cancel.
pub async fn pick_text_file(accept: &str) -> Option<(String, String)> {
    let (tx, rx) = kairo_core::tokio::sync::oneshot::channel::<(String, String)>();
    let document = web_sys::window()?.document()?;
    let input: web_sys::HtmlInputElement = document.create_element("input").ok()?.dyn_into().ok()?;
    input.set_type("file");
    input.set_accept(accept);
    let tx = Rc::new(RefCell::new(Some(tx)));
    let field = input.clone();
    let onchange = Closure::<dyn FnMut()>::new(move || {
        let Some(file) = field.files().and_then(|l| l.get(0)) else { return };
        let name = file.name();
        let tx = tx.clone();
        let read = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(move |v: wasm_bindgen::JsValue| {
            if let (Some(text), Some(tx)) = (v.as_string(), tx.borrow_mut().take()) {
                let _ = tx.send((name.clone(), text));
            }
        });
        let _ = file.text().then(&read);
        read.forget();
    });
    input.set_onchange(Some(onchange.as_ref().unchecked_ref()));
    onchange.forget();
    input.click();
    rx.await.ok()
}
