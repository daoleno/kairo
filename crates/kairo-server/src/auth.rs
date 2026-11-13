//! Who may use the page: one password (argon2, in `<home>/web/password`),
//! and the browsers signed in with it (in `<home>/web/sessions.json`, by the
//! SHA-256 of their token: the file alone signs no one in).
//!
//! - No password yet: the server makes a one-time setup code at each start
//!   (printed, and kept in `<home>/web/setup-code` for `kairo-server status`);
//!   the first page sets the password with it. There is no default password.
//! - A session is a random 256-bit token in an HttpOnly cookie. It lasts 30
//!   days from its last use, survives restarts, and ends when signed out,
//!   revoked from another browser, or by `kairo-server password` (which
//!   ends them all).
//! - Tries are limited ([`Limiter`]): per address (IPv6 per /64), per
//!   browser that signed in before (a device cookie, its SHA-256 kept in
//!   `<home>/web/devices.json`), and everyone else together; a password is
//!   checked one at a time (argon2 takes memory).
//!
//! `kairo-server password` and the server are two processes on one folder:
//! the server reads the files again when they change.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine;
use kairo_core::protocol::SessionInfo;
use kairo_core::setup_code;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, broadcast};

/// A session lasts this long after its last use.
pub const SESSION_DAYS: i64 = 30;
pub const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 1024;
/// The most a typed setup code may be (it is 14 characters; room for spaces).
const MAX_CODE_TYPED: usize = 256;

/// A signed-in browser: the SHA-256 of its token, in hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    /// What Settings shows and revokes it by (not the token, nor its hash).
    id: String,
    agent: String,
    address: String,
    created: i64,
    last_seen: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    /// The password is wrong (sign-in).
    WrongPassword,
    /// The setup code is wrong (setting the first password).
    WrongCode,
    /// Too many tries: wait this long.
    Wait(Duration),
    /// A password exists already (setup is over).
    AlreadySet,
    /// No password yet (sign-in is not possible).
    NotSet,
    TooShort,
    Io(String),
}

impl AuthError {
    pub fn message(&self) -> String {
        match self {
            AuthError::WrongPassword => "Wrong password".into(),
            AuthError::WrongCode => "Wrong code: compare it with what kairo-server status shows".into(),
            AuthError::Wait(d) => format!("Too many tries. Try again in {} s", d.as_secs().max(1)),
            AuthError::AlreadySet => "A password is set already: sign in".into(),
            AuthError::NotSet => "No password yet: set one first".into(),
            AuthError::TooShort => format!("The new password needs at least {MIN_PASSWORD} characters"),
            AuthError::Io(e) => e.clone(),
        }
    }
}

/// A browser signed in: its session token, and its device token (for the
/// device cookie: the next sign-in from this browser is never held by others).
#[derive(Debug, PartialEq, Eq)]
pub struct SignedIn {
    pub token: String,
    pub device: String,
}

/// Browsers and addresses that signed in before (by SHA-256 and by the
/// address as counted), with when: the newest [`MAX_KNOWN`] of each.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Known {
    #[serde(default)]
    devices: HashMap<String, i64>,
    #[serde(default)]
    addresses: HashMap<String, i64>,
}

const MAX_KNOWN: usize = 64;

fn keep_newest(map: &mut HashMap<String, i64>) {
    while map.len() > MAX_KNOWN {
        let Some(oldest) = map.iter().min_by_key(|(_, t)| **t).map(|(k, _)| k.clone()) else { break };
        map.remove(&oldest);
    }
}

pub struct Auth {
    dir: PathBuf,
    inner: Mutex<Inner>,
    known: Mutex<Known>,
    limiter: Mutex<Limiter>,
    /// argon2 runs one at a time: a flood of tries cannot take the memory.
    verify: Semaphore,
    /// Sessions that ended, for their open sockets to close.
    ended: broadcast::Sender<SessionKey>,
}

struct Inner {
    password: Option<String>,
    sessions: HashMap<String, Record>,
    setup_code: Option<String>,
    /// When the files were last read (to see the CLI's changes).
    password_mtime: Option<SystemTime>,
    sessions_mtime: Option<SystemTime>,
    /// Last-seen times not yet written (written at most hourly).
    dirty_since: Option<Instant>,
}

fn now() -> i64 {
    kairo_core::unix()
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn hex_sha256(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    let (a, b) = (Sha256::digest(a), Sha256::digest(b));
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Writes `bytes` to `path` readable by its owner only.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(tmp, path)
}

/// A setup code: twelve letters and digits nobody misreads, in fours
/// ([`setup_code`]).
fn new_code() -> String {
    let mut rng = rand::rng();
    let chars: String = (0..setup_code::LENGTH)
        .map(|_| setup_code::ALPHABET[rng.random_range(0..setup_code::ALPHABET.len())] as char)
        .collect();
    setup_code::grouped(&chars)
}

fn new_token() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The argon2 hash of `password` (PHC string).
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt_bytes: [u8; 16] = rand::rng().random();
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|e| AuthError::Io(e.to_string()))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AuthError::Io(e.to_string()))
}

fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

fn check_new_password(password: &str) -> Result<(), AuthError> {
    let n = password.chars().count();
    if n < MIN_PASSWORD {
        return Err(AuthError::TooShort);
    }
    if password.len() > MAX_PASSWORD {
        return Err(AuthError::Io("The new password is too long".into()));
    }
    Ok(())
}

/// A try in flight: answered once, and counted wrong if never answered (the
/// browser went away during the check): no way out leaves it in flight.
struct Answer<'a> {
    limiter: &'a Mutex<Limiter>,
    ticket: Option<Ticket>,
}

impl Answer<'_> {
    fn finish(mut self, outcome: Outcome) {
        self.answer(outcome);
    }
    fn answer(&mut self, outcome: Outcome) {
        if let Some(t) = self.ticket.take() {
            self.limiter.lock().unwrap_or_else(|e| e.into_inner()).finish(t, outcome, Instant::now());
        }
    }
}

impl Drop for Answer<'_> {
    fn drop(&mut self) {
        self.answer(Outcome::Wrong);
    }
}

impl Auth {
    /// The folder's state; a setup code is made when there is no password.
    pub fn open(dir: impl Into<PathBuf>) -> std::io::Result<Auth> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let auth = Auth {
            dir,
            inner: Mutex::new(Inner {
                password: None,
                sessions: HashMap::new(),
                setup_code: None,
                password_mtime: None,
                sessions_mtime: None,
                dirty_since: None,
            }),
            known: Mutex::new(Known::default()),
            limiter: Mutex::new(Limiter::default()),
            verify: Semaphore::new(1),
            ended: broadcast::channel(64).0,
        };
        *auth.known.lock().unwrap_or_else(|e| e.into_inner()) = std::fs::read(auth.devices_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        {
            let mut inner = auth.lock();
            auth.reload(&mut inner);
            if inner.password.is_none() {
                let code = new_code();
                write_private(&auth.code_file(), format!("{code}\n").as_bytes())?;
                inner.setup_code = Some(code);
            } else {
                let _ = std::fs::remove_file(auth.code_file());
            }
        }
        Ok(auth)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn password_file(&self) -> PathBuf {
        self.dir.join("password")
    }
    fn sessions_file(&self) -> PathBuf {
        self.dir.join("sessions.json")
    }
    pub fn code_file(&self) -> PathBuf {
        self.dir.join("setup-code")
    }
    fn devices_file(&self) -> PathBuf {
        self.dir.join("devices.json")
    }

    /// Who is trying: a browser that signed in before (its device cookie), or
    /// an address; and whether that one signed in before.
    fn trier(&self, ip: IpAddr, device: Option<&str>) -> (Trier, bool) {
        let known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = device.filter(|d| !d.is_empty() && d.len() <= 128) {
            let key = hex_sha256(d.as_bytes());
            if known.devices.contains_key(&key) {
                return (Trier::Device(key), true);
            }
        }
        let net = Trier::net(ip);
        let seen = known.addresses.contains_key(&net.to_string());
        (Trier::Net(net), seen)
    }

    /// Remembers this browser and address: the device token to set (the one
    /// it had, if known, so its cookie stays).
    fn remember(&self, ip: IpAddr, device: Option<&str>) -> String {
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        let t = now();
        let kept = device
            .filter(|d| !d.is_empty() && d.len() <= 128)
            .filter(|d| known.devices.contains_key(&hex_sha256(d.as_bytes())))
            .map(str::to_owned);
        let token = kept.unwrap_or_else(new_token);
        known.devices.insert(hex_sha256(token.as_bytes()), t);
        known.addresses.insert(Trier::net(ip).to_string(), t);
        keep_newest(&mut known.devices);
        keep_newest(&mut known.addresses);
        if let Ok(bytes) = serde_json::to_vec_pretty(&*known)
            && let Err(e) = write_private(&self.devices_file(), &bytes)
        {
            log::warn!("cannot keep the signed-in browsers: {e}");
        }
        token
    }

    /// Lets a try through, or says how long to wait.
    fn limited(&self, trier: Trier, known: bool) -> Result<Answer<'_>, AuthError> {
        let ticket = self
            .limiter
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .allow(trier, known, Instant::now())
            .map_err(AuthError::Wait)?;
        Ok(Answer { limiter: &self.limiter, ticket: Some(ticket) })
    }

    /// Reads the files again if another process (the CLI) changed them.
    fn reload(&self, inner: &mut Inner) {
        let pm = mtime(&self.password_file());
        if pm != inner.password_mtime || (pm.is_none() && inner.password.is_some()) {
            inner.password = std::fs::read_to_string(self.password_file())
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty());
            inner.password_mtime = pm;
            if inner.password.is_some() {
                inner.setup_code = None;
            }
        }
        let sm = mtime(&self.sessions_file());
        if sm != inner.sessions_mtime {
            let before: Vec<String> = inner.sessions.keys().cloned().collect();
            inner.sessions = std::fs::read(self.sessions_file())
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            inner.sessions_mtime = sm;
            for key in before {
                if !inner.sessions.contains_key(&key) {
                    let _ = self.ended.send(SessionKey(key));
                }
            }
        }
    }

    fn save_sessions(&self, inner: &mut Inner) -> Result<(), AuthError> {
        let bytes = serde_json::to_vec_pretty(&inner.sessions).map_err(|e| AuthError::Io(e.to_string()))?;
        write_private(&self.sessions_file(), &bytes).map_err(|e| AuthError::Io(e.to_string()))?;
        inner.sessions_mtime = mtime(&self.sessions_file());
        inner.dirty_since = None;
        Ok(())
    }

    pub fn needs_setup(&self) -> bool {
        let mut inner = self.lock();
        self.reload(&mut inner);
        inner.password.is_none()
    }

    pub fn setup_code(&self) -> Option<String> {
        self.lock().setup_code.clone()
    }

    /// Ends sessions every socket of which must close.
    pub fn ended(&self) -> broadcast::Receiver<SessionKey> {
        self.ended.subscribe()
    }

    fn start_session(&self, inner: &mut Inner, agent: &str, address: &str) -> Result<String, AuthError> {
        let token = new_token();
        let key = hex_sha256(token.as_bytes());
        let t = now();
        inner.sessions.insert(
            key.clone(),
            Record {
                id: key[..16].to_owned(),
                agent: agent.chars().take(160).collect(),
                address: address.to_owned(),
                created: t,
                last_seen: t,
            },
        );
        self.save_sessions(inner)?;
        Ok(token)
    }

    /// Sets the first password with the setup code; signs this browser in.
    pub async fn setup(
        &self,
        code: &str,
        password: &str,
        ip: IpAddr,
        device: Option<&str>,
        agent: &str,
    ) -> Result<SignedIn, AuthError> {
        check_new_password(password)?;
        let (trier, known) = self.trier(ip, device);
        let ticket = self.limited(trier, known)?;
        let result = self.setup_checked(code, password, ip, agent).await;
        ticket.finish(outcome(&result));
        let token = result?;
        Ok(SignedIn { token, device: self.remember(ip, device) })
    }

    async fn setup_checked(&self, code: &str, password: &str, ip: IpAddr, agent: &str) -> Result<String, AuthError> {
        let expected = {
            let mut inner = self.lock();
            self.reload(&mut inner);
            if inner.password.is_some() {
                return Err(AuthError::AlreadySet);
            }
            inner.setup_code.clone()
        };
        // By the code's characters alone, as the page reads it: a space, a
        // dash of another kind or a stray I is not a wrong code. One try is
        // still one code of 12 characters of 32 (60 bits).
        if code.len() > MAX_CODE_TYPED {
            return Err(AuthError::WrongCode);
        }
        let given = setup_code::normalize(code);
        if !expected.is_some_and(|c| constant_eq(setup_code::normalize(&c).as_bytes(), given.as_bytes())) {
            return Err(AuthError::WrongCode);
        }
        let _permit = self.verify.acquire().await.map_err(|e| AuthError::Io(e.to_string()))?;
        let p = password.to_owned();
        let hash = tokio::task::spawn_blocking(move || hash_password(&p))
            .await
            .map_err(|e| AuthError::Io(e.to_string()))??;
        let mut inner = self.lock();
        self.reload(&mut inner);
        if inner.password.is_some() {
            return Err(AuthError::AlreadySet);
        }
        write_private(&self.password_file(), hash.as_bytes()).map_err(|e| AuthError::Io(e.to_string()))?;
        inner.password = Some(hash);
        inner.password_mtime = mtime(&self.password_file());
        inner.setup_code = None;
        let _ = std::fs::remove_file(self.code_file());
        self.start_session(&mut inner, agent, &ip.to_string())
    }

    /// Signs a browser in: its new session token.
    pub async fn login(
        &self,
        password: &str,
        ip: IpAddr,
        device: Option<&str>,
        agent: &str,
    ) -> Result<SignedIn, AuthError> {
        let (trier, known) = self.trier(ip, device);
        let ticket = self.limited(trier, known)?;
        let result = self.login_checked(password, ip, agent).await;
        ticket.finish(outcome(&result));
        let token = result?;
        Ok(SignedIn { token, device: self.remember(ip, device) })
    }

    async fn login_checked(&self, password: &str, ip: IpAddr, agent: &str) -> Result<String, AuthError> {
        let hash = {
            let mut inner = self.lock();
            self.reload(&mut inner);
            inner.password.clone().ok_or(AuthError::NotSet)?
        };
        if password.len() > MAX_PASSWORD {
            return Err(AuthError::WrongPassword);
        }
        let _permit = self.verify.acquire().await.map_err(|e| AuthError::Io(e.to_string()))?;
        let p = password.to_owned();
        let ok = tokio::task::spawn_blocking(move || verify_password(&hash, &p))
            .await
            .unwrap_or(false);
        if !ok {
            return Err(AuthError::WrongPassword);
        }
        let mut inner = self.lock();
        self.start_session(&mut inner, agent, &ip.to_string())
    }

    /// The session of a cookie's token, if it is a live one.
    pub fn check(&self, token: &str) -> Option<SessionKey> {
        if token.is_empty() || token.len() > 128 {
            return None;
        }
        let key = hex_sha256(token.as_bytes());
        let mut inner = self.lock();
        self.reload(&mut inner);
        let t = now();
        let rec = inner.sessions.get_mut(&key)?;
        if t - rec.last_seen > SESSION_DAYS * 86_400 {
            inner.sessions.remove(&key);
            let _ = self.save_sessions(&mut inner);
            return None;
        }
        if t - rec.last_seen > 60 {
            rec.last_seen = t;
            if inner.dirty_since.is_none() {
                inner.dirty_since = Some(Instant::now());
            }
        }
        if inner.dirty_since.is_some_and(|d| d.elapsed() > Duration::from_secs(3600)) {
            let _ = self.save_sessions(&mut inner);
        }
        Some(SessionKey(key))
    }

    /// Whether `key` is still signed in (open sockets ask now and then).
    pub fn alive(&self, key: &SessionKey) -> bool {
        let mut inner = self.lock();
        self.reload(&mut inner);
        inner
            .sessions
            .get(&key.0)
            .is_some_and(|r| now() - r.last_seen <= SESSION_DAYS * 86_400)
    }

    pub fn sessions(&self, current: &SessionKey) -> Vec<SessionInfo> {
        let mut inner = self.lock();
        self.reload(&mut inner);
        let mut list: Vec<SessionInfo> = inner
            .sessions
            .iter()
            .map(|(key, r)| SessionInfo {
                id: r.id.clone(),
                agent: r.agent.clone(),
                address: r.address.clone(),
                created: r.created,
                last_seen: r.last_seen,
                current: *key == current.0,
            })
            .collect();
        list.sort_by_key(|s| (!s.current, -s.last_seen));
        list
    }

    fn end(&self, inner: &mut Inner, keys: Vec<String>) {
        for k in keys {
            inner.sessions.remove(&k);
            let _ = self.ended.send(SessionKey(k));
        }
        let _ = self.save_sessions(inner);
    }

    /// Signs out the browser Settings lists as `id`.
    pub fn revoke(&self, id: &str) -> Result<(), String> {
        let mut inner = self.lock();
        self.reload(&mut inner);
        let keys: Vec<String> = inner
            .sessions
            .iter()
            .filter(|(_, r)| r.id == id)
            .map(|(k, _)| k.clone())
            .collect();
        if keys.is_empty() {
            return Err("That browser is signed out already".into());
        }
        self.end(&mut inner, keys);
        Ok(())
    }

    pub fn revoke_others(&self, current: &SessionKey) {
        let mut inner = self.lock();
        self.reload(&mut inner);
        let keys = inner.sessions.keys().filter(|k| **k != current.0).cloned().collect();
        self.end(&mut inner, keys);
    }

    pub fn sign_out(&self, current: &SessionKey) {
        let mut inner = self.lock();
        self.end(&mut inner, vec![current.0.clone()]);
    }

    /// `kairo-server password`: a new password; every browser signs in again.
    pub fn reset_password(&self, password: &str) -> Result<(), AuthError> {
        check_new_password(password)?;
        let hash = hash_password(password)?;
        let mut inner = self.lock();
        write_private(&self.password_file(), hash.as_bytes()).map_err(|e| AuthError::Io(e.to_string()))?;
        inner.password = Some(hash);
        inner.password_mtime = mtime(&self.password_file());
        inner.setup_code = None;
        let _ = std::fs::remove_file(self.code_file());
        let keys = inner.sessions.keys().cloned().collect();
        self.end(&mut inner, keys);
        Ok(())
    }

    /// Writes the last-seen times kept in memory (at a clean stop).
    pub fn flush(&self) {
        let mut inner = self.lock();
        if inner.dirty_since.is_some() {
            let _ = self.save_sessions(&mut inner);
        }
    }

}

/// How many wrong passwords one trier (an address, or a browser that signed
/// in before) may try before it waits.
const FREE_TRIES: u32 = 5;
/// The longest a trier waits.
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);
/// Tries of everyone else together: a bucket this deep...
const GLOBAL_BURST: f64 = 30.0;
/// ...that fills by one try a minute (so at most about 1,500 a day).
const GLOBAL_REFILL: Duration = Duration::from_secs(60);
/// Triers remembered at once (the oldest go first).
const MAX_TRIERS: usize = 10_000;

/// Who is trying a password.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Trier {
    /// A browser that signed in before: the SHA-256 of its device token.
    Device(String),
    /// Anyone else, by address (IPv6 by its /64: one machine has many).
    Net(IpAddr),
}

impl Trier {
    /// The address as counted: an IPv4-mapped address as IPv4, IPv6 as its /64.
    pub fn net(ip: IpAddr) -> IpAddr {
        match ip {
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => IpAddr::V4(v4),
                None => {
                    let s = v6.segments();
                    IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
                }
            },
            v4 => v4,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Tries {
    /// Wrong ones since the last right one.
    failures: u32,
    /// Ones being checked now (counted before the answer: tries sent at
    /// once cannot pass the limit together).
    pending: u32,
    last: Instant,
}

/// What a try let through came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Right,
    Wrong,
    /// Neither (no password yet, one set already, a failure of the server):
    /// it counts neither way.
    Neither,
}

fn outcome<T>(r: &Result<T, AuthError>) -> Outcome {
    match r {
        Ok(_) => Outcome::Right,
        Err(AuthError::WrongPassword | AuthError::WrongCode) => Outcome::Wrong,
        Err(_) => Outcome::Neither,
    }
}

/// A try let through: its answer goes back with [`Limiter::finish`].
#[derive(Debug)]
pub struct Ticket {
    trier: Trier,
    /// It took from the shared bucket (given back if it was right).
    shared: bool,
}

/// Sign-in tries.
///
/// - Every trier gets [`FREE_TRIES`] wrong ones, then waits 30 s, doubling
///   with each further wrong one (at most [`MAX_WAIT`]). A try counts from
///   the moment it starts, so tries sent at once count too.
/// - A browser that signed in before (its device cookie) is its own trier
///   and is never held by what others do: not by its address, not by the
///   shared bucket. Nor is an address that signed in before.
/// - Everyone else shares one bucket of [`GLOBAL_BURST`] tries, filled by
///   one a [`GLOBAL_REFILL`]: guessing from many addresses is bounded to
///   about 1,500 tries a day, whoever spreads them.
pub struct Limiter {
    tries: HashMap<Trier, Tries>,
    shared: f64,
    filled: Instant,
}

impl Default for Limiter {
    fn default() -> Self {
        Limiter { tries: HashMap::new(), shared: GLOBAL_BURST, filled: Instant::now() }
    }
}

impl Limiter {
    /// Lets a try through, or says how long to wait. `known`: the trier
    /// signed in before (a device, or an address that did).
    pub fn allow(&mut self, trier: Trier, known: bool, now: Instant) -> Result<Ticket, Duration> {
        if let Some(t) = self.tries.get(&trier).copied() {
            // Quiet for a day, with nothing in flight: it starts over.
            if t.pending == 0 && now.duration_since(t.last) > Duration::from_secs(86_400) {
                self.tries.remove(&trier);
            } else {
                let attempts = t.failures + t.pending;
                if attempts >= FREE_TRIES {
                    let wait = (Duration::from_secs(30) * 2u32.saturating_pow(attempts - FREE_TRIES)).min(MAX_WAIT);
                    let passed = now.duration_since(t.last);
                    if passed < wait {
                        return Err(wait - passed);
                    }
                }
            }
        }
        let shared = !known;
        if shared {
            let refilled = now.duration_since(self.filled).as_secs_f64() / GLOBAL_REFILL.as_secs_f64();
            self.shared = (self.shared + refilled).min(GLOBAL_BURST);
            self.filled = now;
            if self.shared < 1.0 {
                return Err(GLOBAL_REFILL.mul_f64(1.0 - self.shared).max(Duration::from_secs(1)));
            }
            self.shared -= 1.0;
        }
        let t = self.tries.entry(trier.clone()).or_insert(Tries { failures: 0, pending: 0, last: now });
        t.pending += 1;
        t.last = now;
        if self.tries.len() > MAX_TRIERS
            && let Some(oldest) = self.tries.iter().filter(|(_, t)| t.pending == 0).min_by_key(|(_, t)| t.last).map(|(k, _)| k.clone())
        {
            self.tries.remove(&oldest);
        }
        Ok(Ticket { trier, shared })
    }

    /// The answer to a try let through: right clears its trier, wrong counts.
    pub fn finish(&mut self, ticket: Ticket, outcome: Outcome, now: Instant) {
        if outcome == Outcome::Right {
            self.tries.remove(&ticket.trier);
            if ticket.shared {
                self.shared = (self.shared + 1.0).min(GLOBAL_BURST);
            }
            return;
        }
        if let Some(t) = self.tries.get_mut(&ticket.trier) {
            t.pending = t.pending.saturating_sub(1);
            if outcome == Outcome::Wrong {
                t.failures += 1;
                t.last = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([192, 168, 1, n])
    }

    fn net(n: u8) -> Trier {
        Trier::Net(ip(n))
    }

    fn wrong(l: &mut Limiter, who: Trier, known: bool, t: Instant) -> Result<(), Duration> {
        let ticket = l.allow(who, known, t)?;
        l.finish(ticket, Outcome::Wrong, t);
        Ok(())
    }

    #[test]
    fn an_address_waits_after_five_wrong_tries_and_others_do_not() {
        let mut l = Limiter::default();
        let t = Instant::now();
        for _ in 0..FREE_TRIES {
            wrong(&mut l, net(1), false, t).unwrap();
        }
        let wait = l.allow(net(1), false, t).unwrap_err();
        assert_eq!(wait, Duration::from_secs(30));
        assert!(wrong(&mut l, net(2), false, t).is_ok(), "another address is not held");
        // After the wait, one more try; wrong again, the wait doubles.
        let later = t + Duration::from_secs(31);
        wrong(&mut l, net(1), false, later).unwrap();
        assert_eq!(l.allow(net(1), false, later).unwrap_err(), Duration::from_secs(60));
        // Right: free again.
        let after = later + Duration::from_secs(61);
        let ticket = l.allow(net(1), false, after).unwrap();
        l.finish(ticket, Outcome::Right, after);
        assert!(l.allow(net(1), false, after).is_ok());
    }

    #[test]
    fn tries_at_once_cannot_pass_the_limit_together() {
        let mut l = Limiter::default();
        let t = Instant::now();
        // Twenty tries sent at once: none answered yet.
        let let_through: Vec<Ticket> = (0..20).filter_map(|_| l.allow(net(1), false, t).ok()).collect();
        assert_eq!(let_through.len(), FREE_TRIES as usize);
        // An answer that is neither right nor wrong frees its place, no more.
        let mut it = let_through.into_iter();
        l.finish(it.next().unwrap(), Outcome::Neither, t);
        assert!(l.allow(net(1), false, t).is_ok());
        assert!(l.allow(net(1), false, t).is_err());
    }

    #[test]
    fn ipv6_counts_by_its_64() {
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(Trier::net(a), Trier::net(b));
        assert_ne!(Trier::net(a), Trier::net(c));
        let mapped: IpAddr = "::ffff:192.168.1.1".parse().unwrap();
        assert_eq!(Trier::net(mapped), ip(1));
        let mut l = Limiter::default();
        let t = Instant::now();
        for i in 0..FREE_TRIES {
            let one = std::net::Ipv6Addr::new(0x2001, 0xdb8, 1, 2, 0, 0, 0, i as u16 + 1);
            wrong(&mut l, Trier::Net(Trier::net(IpAddr::V6(one))), false, t).unwrap();
        }
        assert!(l.allow(Trier::Net(Trier::net(b)), false, t).is_err(), "the same /64");
        assert!(l.allow(Trier::Net(Trier::net(c)), false, t).is_ok(), "another /64");
    }

    #[test]
    fn everyone_unknown_together_shares_one_slow_bucket() {
        let mut l = Limiter::default();
        let t = Instant::now();
        for i in 0..GLOBAL_BURST as u32 {
            wrong(&mut l, Trier::Net(IpAddr::from(std::net::Ipv4Addr::from((10u32 << 24) + i))), false, t).unwrap();
        }
        assert!(l.allow(net(250), false, t).is_err(), "the bucket is empty");
        assert!(l.allow(net(250), false, t + Duration::from_secs(61)).is_ok(), "one a minute");
        // Over a day of guessing from ever new addresses, at most this many tries.
        let mut l = Limiter::default();
        let mut tries = 0u32;
        for s in 0..86_400u32 {
            let now = t + Duration::from_secs(s as u64);
            if wrong(&mut l, Trier::Net(IpAddr::from(std::net::Ipv4Addr::from(s))), false, now).is_ok() {
                tries += 1;
            }
        }
        assert!(tries <= 30 + 1440, "{tries} tries a day");
    }

    #[test]
    fn nobody_keeps_a_known_browser_or_address_out() {
        let mut l = Limiter::default();
        let t = Instant::now();
        let mine = Trier::Device("my browser".into());
        // Others empty the shared bucket, and wear out my address.
        for i in 0..200u32 {
            let _ = wrong(&mut l, Trier::Net(IpAddr::from(std::net::Ipv4Addr::from((10u32 << 24) + i))), false, t);
        }
        for _ in 0..50 {
            let _ = wrong(&mut l, net(1), false, t);
        }
        assert!(l.allow(net(1), false, t).is_err());
        // My browser (its device cookie) still signs in from that address.
        let ticket = l.allow(mine.clone(), true, t).unwrap();
        l.finish(ticket, Outcome::Right, t);
        // An address that signed in before is not held by the shared bucket.
        assert!(l.allow(net(7), true, t).is_ok());
        assert!(l.allow(net(8), false, t).is_err());
        // My browser's own wrong tries still count, for it alone.
        for _ in 0..FREE_TRIES {
            wrong(&mut l, mine.clone(), true, t).unwrap();
        }
        assert!(l.allow(mine, true, t).is_err());
    }

    #[tokio::test]
    async fn a_try_cut_short_counts_and_does_not_stay_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let auth = Auth::open(dir.path().join("web")).unwrap();
        auth.reset_password("long enough").unwrap();
        for _ in 0..FREE_TRIES {
            let ticket = auth.limited(net(1), false).unwrap();
            drop(ticket);
        }
        assert!(matches!(auth.login("long enough", ip(1), None, "t").await, Err(AuthError::Wait(_))));
    }

    #[tokio::test]
    async fn a_signed_in_browser_gets_its_device_back() {
        let dir = tempfile::tempdir().unwrap();
        let auth = Auth::open(dir.path().join("web")).unwrap();
        auth.reset_password("long enough").unwrap();
        let first = auth.login("long enough", ip(1), None, "t").await.unwrap();
        let again = auth.login("long enough", ip(2), Some(&first.device), "t").await.unwrap();
        assert_eq!(first.device, again.device, "the cookie stays");
        let other = auth.login("long enough", ip(2), Some("made up"), "t").await.unwrap();
        assert_ne!(other.device, "made up", "an unknown device is not taken");
        // Kept across a restart, by hash only.
        let file = std::fs::read_to_string(dir.path().join("web/devices.json")).unwrap();
        assert!(!file.contains(&first.device));
        let restarted = Auth::open(dir.path().join("web")).unwrap();
        assert!(matches!(restarted.trier(ip(9), Some(&first.device)), (Trier::Device(_), true)));
        assert!(matches!(restarted.trier(ip(1), None), (Trier::Net(_), true)));
        assert!(matches!(restarted.trier(ip(9), None), (Trier::Net(_), false)));
    }

    #[test]
    fn a_session_past_its_days_is_not_alive() {
        let dir = tempfile::tempdir().unwrap();
        let auth = Auth::open(dir.path().join("web")).unwrap();
        let key = "k".repeat(64);
        {
            let mut inner = auth.lock();
            inner.sessions.insert(
                key.clone(),
                Record { id: "x".into(), agent: "t".into(), address: "a".into(), created: 0, last_seen: now() },
            );
        }
        assert!(auth.alive(&SessionKey(key.clone())));
        auth.lock().sessions.get_mut(&key).unwrap().last_seen = now() - SESSION_DAYS * 86_400 - 1;
        assert!(!auth.alive(&SessionKey(key)), "an open socket ends with its session");
    }

    #[tokio::test]
    async fn setup_then_sign_in_then_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let auth = Auth::open(dir.path().join("web")).unwrap();
        assert!(auth.needs_setup());
        let code = auth.setup_code().unwrap();
        assert_eq!(
            std::fs::read_to_string(auth.code_file()).unwrap().trim(),
            code,
            "status can show it"
        );
        let me = ip(1);
        assert_eq!(auth.setup("WRONG-CODE-0000", "long enough", me, None, "t").await, Err(AuthError::WrongCode));
        assert_eq!(auth.setup(&code, "short", me, None, "t").await, Err(AuthError::TooShort));
        // A code with one character changed is wrong, however it is typed.
        let mut other: Vec<char> = code.chars().collect();
        other[0] = if other[0] == 'A' { 'B' } else { 'A' };
        let other: String = other.into_iter().collect();
        assert_eq!(auth.setup(&other.to_lowercase(), "long enough", me, None, "t").await, Err(AuthError::WrongCode));
        // As a phone may type it: lower case, a space for each dash, a stray I.
        let typed = format!("{} I", code.to_lowercase().replace('-', " "));
        let first = auth.setup(&typed, "long enough", me, None, "t").await.unwrap().token;
        assert!(!auth.code_file().exists(), "the code is gone once used");
        assert_eq!(auth.setup(&code, "another one", me, None, "t").await, Err(AuthError::AlreadySet));
        let a = auth.check(&first).expect("signed in");
        assert_eq!(auth.login("wrong password", me, None, "t").await.map(|s| s.token), Err(AuthError::WrongPassword));
        let second = auth.login("long enough", me, None, "phone").await.unwrap().token;
        let b = auth.check(&second).unwrap();
        assert_ne!(a, b);
        // The files hold no token.
        let file = std::fs::read_to_string(dir.path().join("web/sessions.json")).unwrap();
        assert!(!file.contains(&first) && !file.contains(&second));
        // A restart keeps both.
        let again = Auth::open(dir.path().join("web")).unwrap();
        assert!(again.check(&first).is_some() && again.check(&second).is_some());
        assert!(again.setup_code().is_none());
        // Revoked from the first browser: the second is out, and its sockets hear.
        let mut ended = again.ended();
        let id = again.sessions(&a).into_iter().find(|s| !s.current).unwrap().id;
        again.revoke(&id).unwrap();
        assert_eq!(ended.recv().await.unwrap(), b);
        assert!(again.check(&second).is_none());
        assert!(again.check(&first).is_some());
        // The CLI's new password ends every session, seen by a running server.
        again.reset_password("a new password").unwrap();
        assert!(auth.check(&first).is_none(), "the other process sees the change");
        assert_eq!(auth.login("long enough", ip(9), None, "t").await.map(|s| s.token), Err(AuthError::WrongPassword));
        assert!(auth.login("a new password", ip(9), None, "t").await.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn the_files_are_the_owners_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let auth = Auth::open(dir.path().join("web")).unwrap();
        auth.reset_password("long enough").unwrap();
        for f in ["web", "web/password", "web/sessions.json"] {
            let mode = std::fs::metadata(dir.path().join(f)).unwrap().permissions().mode() & 0o077;
            assert_eq!(mode, 0, "{f}");
        }
    }
}
