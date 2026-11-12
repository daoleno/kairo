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
//! - Tries are limited: per address, and in all ([`Limiter`]); a password
//!   is checked one at a time (argon2 takes memory).
//!
//! `kairo-server password` and the server are two processes on one folder:
//! the server reads the files again when they change.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine;
use kairo_core::protocol::SessionInfo;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, broadcast};

/// A session lasts this long after its last use.
pub const SESSION_DAYS: i64 = 30;
pub const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 1024;

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
    /// The password (or code) is wrong.
    Wrong,
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
            AuthError::Wrong => "Wrong password".into(),
            AuthError::Wait(d) => format!("Too many tries. Try again in {} s", d.as_secs().max(1)),
            AuthError::AlreadySet => "A password is set already: sign in".into(),
            AuthError::NotSet => "No password yet: set one first".into(),
            AuthError::TooShort => format!("Use at least {MIN_PASSWORD} characters"),
            AuthError::Io(e) => e.clone(),
        }
    }
}

pub struct Auth {
    dir: PathBuf,
    inner: Mutex<Inner>,
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

/// A setup code: twelve letters and digits nobody misreads, in threes.
fn new_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::rng();
    let chars: Vec<char> = (0..12)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect();
    chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
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
        return Err(AuthError::Io("That password is too long".into()));
    }
    Ok(())
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
            limiter: Mutex::new(Limiter::default()),
            verify: Semaphore::new(1),
            ended: broadcast::channel(64).0,
        };
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
    pub async fn setup(&self, code: &str, password: &str, ip: IpAddr, agent: &str) -> Result<String, AuthError> {
        self.limited(ip)?;
        check_new_password(password)?;
        let expected = {
            let mut inner = self.lock();
            self.reload(&mut inner);
            if inner.password.is_some() {
                return Err(AuthError::AlreadySet);
            }
            inner.setup_code.clone()
        };
        let given = code.trim().to_ascii_uppercase().replace(' ', "");
        if !expected.is_some_and(|c| constant_eq(c.as_bytes(), given.as_bytes())) {
            self.failed(ip);
            return Err(AuthError::Wrong);
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
        self.succeeded(ip);
        self.start_session(&mut inner, agent, &ip.to_string())
    }

    /// Signs a browser in: its new session token.
    pub async fn login(&self, password: &str, ip: IpAddr, agent: &str) -> Result<String, AuthError> {
        self.limited(ip)?;
        let hash = {
            let mut inner = self.lock();
            self.reload(&mut inner);
            inner.password.clone().ok_or(AuthError::NotSet)?
        };
        if password.len() > MAX_PASSWORD {
            self.failed(ip);
            return Err(AuthError::Wrong);
        }
        let _permit = self.verify.acquire().await.map_err(|e| AuthError::Io(e.to_string()))?;
        let p = password.to_owned();
        let ok = tokio::task::spawn_blocking(move || verify_password(&hash, &p))
            .await
            .unwrap_or(false);
        if !ok {
            self.failed(ip);
            return Err(AuthError::Wrong);
        }
        self.succeeded(ip);
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
        inner.sessions.contains_key(&key.0)
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

    fn limited(&self, ip: IpAddr) -> Result<(), AuthError> {
        self.limiter
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .allow(ip, Instant::now())
            .map_err(AuthError::Wait)
    }

    fn failed(&self, ip: IpAddr) {
        self.limiter.lock().unwrap_or_else(|e| e.into_inner()).failed(ip, Instant::now());
    }

    fn succeeded(&self, ip: IpAddr) {
        self.limiter.lock().unwrap_or_else(|e| e.into_inner()).succeeded(ip);
    }
}

/// How many wrong passwords an address may try before it waits.
const FREE_TRIES: u32 = 5;
/// The longest an address waits.
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);
/// Tries of every address together, per minute.
const GLOBAL_PER_MINUTE: usize = 30;

/// Login tries: each address gets [`FREE_TRIES`] wrong ones, then waits
/// 30 s, doubling with each further wrong one (at most [`MAX_WAIT`]); all
/// addresses together get [`GLOBAL_PER_MINUTE`] tries a minute (behind a
/// proxy every browser has one address).
#[derive(Default)]
pub struct Limiter {
    fails: HashMap<IpAddr, (u32, Instant)>,
    recent: VecDeque<Instant>,
}

impl Limiter {
    pub fn allow(&mut self, ip: IpAddr, now: Instant) -> Result<(), Duration> {
        while self.recent.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
            self.recent.pop_front();
        }
        if let Some((n, last)) = self.fails.get(&ip).copied() {
            if n >= FREE_TRIES {
                let wait = (Duration::from_secs(30) * 2u32.saturating_pow(n - FREE_TRIES)).min(MAX_WAIT);
                let passed = now.duration_since(last);
                if passed < wait {
                    return Err(wait - passed);
                }
            }
            // An address quiet for a day starts over.
            if now.duration_since(last) > Duration::from_secs(86_400) {
                self.fails.remove(&ip);
            }
        }
        if self.recent.len() >= GLOBAL_PER_MINUTE {
            let oldest = *self.recent.front().expect("not empty");
            return Err(Duration::from_secs(60).saturating_sub(now.duration_since(oldest)));
        }
        self.recent.push_back(now);
        Ok(())
    }

    pub fn failed(&mut self, ip: IpAddr, now: Instant) {
        let e = self.fails.entry(ip).or_insert((0, now));
        e.0 += 1;
        e.1 = now;
        // Bounded: the oldest addresses go first.
        if self.fails.len() > 10_000 {
            if let Some(oldest) = self.fails.iter().min_by_key(|(_, v)| v.1).map(|(k, _)| *k) {
                self.fails.remove(&oldest);
            }
        }
    }

    pub fn succeeded(&mut self, ip: IpAddr) {
        self.fails.remove(&ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(n: u8) -> IpAddr {
        IpAddr::from([192, 168, 1, n])
    }

    #[test]
    fn an_address_waits_after_five_wrong_tries_and_others_do_not() {
        let mut l = Limiter::default();
        let t = Instant::now();
        for _ in 0..FREE_TRIES {
            assert!(l.allow(ip(1), t).is_ok());
            l.failed(ip(1), t);
        }
        let wait = l.allow(ip(1), t).unwrap_err();
        assert_eq!(wait, Duration::from_secs(30));
        assert!(l.allow(ip(2), t).is_ok(), "another address is not held");
        // After the wait, one more try; wrong again, the wait doubles.
        let later = t + Duration::from_secs(31);
        assert!(l.allow(ip(1), later).is_ok());
        l.failed(ip(1), later);
        assert_eq!(l.allow(ip(1), later).unwrap_err(), Duration::from_secs(60));
        // Right: free again.
        l.succeeded(ip(1));
        assert!(l.allow(ip(1), later).is_ok());
    }

    #[test]
    fn all_addresses_together_are_limited_per_minute() {
        let mut l = Limiter::default();
        let t = Instant::now();
        for i in 0..GLOBAL_PER_MINUTE {
            assert!(l.allow(ip(i as u8), t).is_ok());
        }
        assert!(l.allow(ip(250), t).is_err());
        assert!(l.allow(ip(250), t + Duration::from_secs(61)).is_ok());
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
        assert_eq!(auth.setup("WRONG-CODE-0000", "long enough", me, "t").await, Err(AuthError::Wrong));
        assert_eq!(auth.setup(&code, "short", me, "t").await, Err(AuthError::TooShort));
        let first = auth.setup(&code.to_lowercase(), "long enough", me, "t").await.unwrap();
        assert!(!auth.code_file().exists(), "the code is gone once used");
        assert_eq!(auth.setup(&code, "another one", me, "t").await, Err(AuthError::AlreadySet));
        let a = auth.check(&first).expect("signed in");
        assert_eq!(auth.login("wrong password", me, "t").await, Err(AuthError::Wrong));
        let second = auth.login("long enough", me, "phone").await.unwrap();
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
        assert_eq!(auth.login("long enough", ip(9), "t").await, Err(AuthError::Wrong));
        assert!(auth.login("a new password", ip(9), "t").await.is_ok());
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
