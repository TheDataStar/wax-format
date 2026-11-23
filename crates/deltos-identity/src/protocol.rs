//! The local socket `identityd` answers on, and what each caller may ask.
//!
//! **Wire format:** one JSON object per line each way, over a Unix stream
//! socket; a connection may carry any number of requests in turn. A request
//! is `{"call": "<name>", ...arguments}`; the answer is `{"ok": <value>}` or
//! `{"error": {"code": "<deltd API §7 code>", "detail": "..."}}`.
//!
//! **Callers** (privilege boundary rule 2, Track F §3): the socket is
//! `0660` with group `deltos-identc`, and on every connection the caller's
//! user id is read from the kernel (`SO_PEERCRED`). `deltd` may make the
//! calls in [`DELTD_CALLS`]; `deltos-shelld` only the three
//! in [`SHELLD_CALLS`]. Anyone else is disconnected before a byte is read.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::register::{Ctx, Error, NewProfile, Register};

/// Largest request line accepted (a profile with a long name is ~1 KiB).
pub const MAX_LINE: usize = 16 * 1024;

/// Who is calling, from the kernel's peer credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Deltd,
    Shelld,
}

pub const DELTD_CALLS: &[&str] = &[
    "session",
    "session_for_partition",
    "tap_list",
    "password_allowed",
    "sign_in_password",
    "sign_in_card",
    "sign_in_device",
    "sign_in_tap",
    "sign_out",
    "sign_out_everywhere",
    "issue_card",
    "list_devices",
    "remove_device",
    "create_profile",
    "list_members",
    "change_password",
    "set_password",
    "issue_ticket",
];

/// `deltos-shelld`'s second check (privilege boundary §5 step 6) and the
/// ticket redemption (§6 step 3). Nothing that writes a profile.
pub const SHELLD_CALLS: &[&str] = &["session", "has_permission", "redeem_ticket"];

pub fn allowed(caller: Caller, call: &str) -> bool {
    match caller {
        Caller::Deltd => DELTD_CALLS.contains(&call),
        Caller::Shelld => SHELLD_CALLS.contains(&call),
    }
}

fn arg<T: DeserializeOwned>(req: &Value, name: &str) -> Result<T, Error> {
    serde_json::from_value(req.get(name).cloned().unwrap_or(Value::Null))
        .map_err(|e| Error::Invalid(format!("{name}: {e}")))
}

fn ctx(req: &Value) -> Result<Ctx, Error> {
    match req.get("ctx") {
        None => Ok(Ctx::default()),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| Error::Invalid(format!("ctx: {e}"))),
    }
}

/// Answer one request line from `caller`.
pub fn answer(reg: &Register, caller: Caller, line: &str) -> Value {
    let req: Value = match serde_json::from_str(line) {
        Ok(v @ Value::Object(_)) => v,
        _ => return err(&Error::Invalid("not a JSON object".into())),
    };
    let call = req.get("call").and_then(Value::as_str).unwrap_or("");
    if !allowed(caller, call) {
        return err(&Error::PermissionDenied);
    }
    match dispatch(reg, call, &req) {
        Ok(v) => json!({ "ok": v }),
        Err(e) => err(&e),
    }
}

fn class(q: &Value) -> Result<Option<String>, Error> {
    arg(q, "class")
}

fn err(e: &Error) -> Value {
    json!({ "error": { "code": e.code(), "detail": e.detail() } })
}

fn to<T: serde::Serialize>(v: T) -> Result<Value, Error> {
    serde_json::to_value(v).map_err(|e| Error::Internal(e.to_string()))
}

fn dispatch(r: &Register, call: &str, q: &Value) -> Result<Value, Error> {
    let s = |n: &str| arg::<String>(q, n);
    match call {
        "session" => to(r.session(&s("token")?)?),
        "session_for_partition" => to(r.session_for_partition(&s("partition")?)?),
        "has_permission" => {
            let sess = r.session(&s("token")?)?;
            let scope: Option<String> = arg(q, "scope")?;
            let perm = s("permission")?;
            // a secure-only permission needs a session made over HTTPS;
            // the request's own transport is deltd's to check (rule 9)
            to(sess.is_some_and(|v| {
                v.holds(&perm, scope.as_deref()) && (!crate::policy::is_secure_only(&perm) || v.secure)
            }))
        }
        "tap_list" => to(r.tap_list(&ctx(q)?)?),
        "password_allowed" => to(r.password_allowed(&ctx(q)?, &s("sign_in_name")?)?),
        "sign_in_password" => to(r.sign_in_password(&ctx(q)?, &s("sign_in_name")?, &s("password")?)?),
        "sign_in_card" => to(r.sign_in_card(&ctx(q)?, &s("code")?)?),
        "sign_in_device" => to(r.sign_in_device(&ctx(q)?, &s("device")?)?),
        "sign_in_tap" => to(r.sign_in_tap(&ctx(q)?, &s("profile_id")?)?),
        "sign_out" => to(r.sign_out(&s("token")?, arg::<Option<String>>(q, "forget_device")?.as_deref())?),
        "sign_out_everywhere" => to(r.sign_out_everywhere(&s("token")?)?),
        "issue_card" => to(r.issue_card(&ctx(q)?, &s("token")?, &s("profile_id")?, class(q)?.as_deref())?),
        "list_devices" => to(r.list_devices(&ctx(q)?, &s("token")?, &s("profile_id")?, class(q)?.as_deref())?),
        "remove_device" => {
            to(r.remove_device(&ctx(q)?, &s("token")?, &s("profile_id")?, class(q)?.as_deref(), &s("device_id")?)?)
        }
        "create_profile" => to(r.create_profile(&ctx(q)?, &s("token")?, &arg::<NewProfile>(q, "profile")?)?),
        "list_members" => to(r.list_members(&ctx(q)?, &s("token")?, &s("class")?)?),
        "change_password" => to(r.change_password(&ctx(q)?, &s("token")?, &s("current")?, &s("new")?)?),
        "set_password" => to(r.set_password(&ctx(q)?, &s("token")?, &s("profile_id")?, &s("new")?)?),
        "issue_ticket" => {
            to(r.issue_ticket(&ctx(q)?, &s("token")?, &s("password")?, &s("action")?, &s("args_digest")?)?)
        }
        "redeem_ticket" => to(r.redeem_ticket(&s("token")?, &s("ticket")?, &s("action")?, &s("args_digest")?)?),
        _ => Err(Error::Invalid(format!("no call {call:?}"))),
    }
}

/// The caller's user id, from the kernel.
#[cfg(target_os = "linux")]
pub fn peer_uid(s: &UnixStream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `len` bytes into `cred`, which is that size.
    let rc = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(cred.uid)
}

#[cfg(not(target_os = "linux"))]
pub fn peer_uid(_s: &UnixStream) -> std::io::Result<u32> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "SO_PEERCRED is Linux only"))
}

/// A user's id by name, or `None` if the user does not exist on this box.
pub fn uid_of(name: &str) -> Option<u32> {
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: getpwnam returns null or a pointer to a static passwd record,
    // read immediately and not kept.
    let p = unsafe { libc::getpwnam(c.as_ptr()) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { (*p).pw_uid })
    }
}

/// Which uids are which caller.
#[derive(Debug, Clone, Default)]
pub struct Callers {
    pub deltd: Vec<u32>,
    pub shelld: Vec<u32>,
}

impl Callers {
    fn of(&self, uid: u32) -> Option<Caller> {
        if self.deltd.contains(&uid) {
            Some(Caller::Deltd)
        } else if self.shelld.contains(&uid) {
            Some(Caller::Shelld)
        } else {
            None
        }
    }
}

/// Serve the socket forever: a thread per connection (a box has a handful
/// of local callers, each keeping its connection).
pub fn serve(listener: UnixListener, reg: Arc<Register>, callers: Callers) -> std::io::Result<()> {
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let caller = match peer_uid(&conn).ok().and_then(|u| callers.of(u)) {
            Some(c) => c,
            None => {
                eprintln!("identityd: refused a connection from a caller that is not deltd or shelld");
                continue;
            }
        };
        let reg = Arc::clone(&reg);
        std::thread::spawn(move || {
            let _ = serve_conn(conn, &reg, caller);
        });
    }
    Ok(())
}

fn serve_conn(conn: UnixStream, reg: &Register, caller: Caller) -> std::io::Result<()> {
    let mut w = conn.try_clone()?;
    let mut r = BufReader::new(conn);
    let mut line = String::new();
    loop {
        line.clear();
        let n = (&mut r).take(MAX_LINE as u64 + 1).read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        if n > MAX_LINE || !line.ends_with('\n') {
            let mut v = err(&Error::Invalid("request too long".into())).to_string();
            v.push('\n');
            w.write_all(v.as_bytes())?;
            return Ok(());
        }
        let mut v = answer(reg, caller, line.trim_end()).to_string();
        v.push('\n');
        w.write_all(v.as_bytes())?;
    }
}

use std::io::Read as _;

/// A blocking client for one caller: one connection, reopened on failure.
/// One kept connection to identityd: its reader and writer halves.
type Conn = Option<(BufReader<UnixStream>, UnixStream)>;

pub struct Client {
    path: std::path::PathBuf,
    /// Kept connections, one call at a time each. identityd answers each
    /// connection on its own thread, so a slow password check on one does
    /// not hold up session checks on the others.
    conns: Vec<std::sync::Mutex<Conn>>,
    next: std::sync::atomic::AtomicUsize,
}

/// What a call can fail with on the client side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// identityd answered with an error code (deltd API §7) and detail.
    Refused { code: String, detail: Option<String> },
    /// identityd could not be reached or answered nonsense.
    Unavailable(String),
}

impl Client {
    /// One connection more than identityd runs password checks at once
    /// ([`crate::secret::password_checks_at_once`]), so even with every core
    /// hashing, a session check still finds a free connection.
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self::with_connections(path, crate::secret::password_checks_at_once() + 1)
    }

    pub fn with_connections(path: impl Into<std::path::PathBuf>, n: usize) -> Self {
        Client {
            path: path.into(),
            conns: (0..n.max(1)).map(|_| std::sync::Mutex::new(None)).collect(),
            next: Default::default(),
        }
    }

    /// Calls that hash a password: slow (tens of milliseconds) and queued
    /// in identityd behind each other when many arrive together.
    const SLOW: &'static [&'static str] = &["sign_in_password", "set_password", "change_password", "create_profile"];

    /// Take a free connection if there is one, else wait for the next in turn.
    /// Slow calls never use the first connection, so quick calls (session
    /// checks above all) always have one that no sign-in burst can fill.
    fn conn(&self, slow: bool) -> std::sync::MutexGuard<'_, Conn> {
        let n = self.conns.len();
        let skip = usize::from(slow && n > 1);
        let pool = &self.conns[skip..];
        let start = if slow { self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) } else { 0 };
        for i in 0..pool.len() {
            if let Ok(g) = pool[(start + i) % pool.len()].try_lock() {
                return g;
            }
        }
        pool[start % pool.len()].lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Make one call. Retries once on a fresh connection if the kept one
    /// was closed (identityd restarted).
    pub fn call(&self, req: &Value) -> Result<Value, CallError> {
        let mut line = req.to_string();
        line.push('\n');
        let slow = req.get("call").and_then(Value::as_str).is_some_and(|c| Self::SLOW.contains(&c));
        let mut g = self.conn(slow);
        for attempt in 0..2 {
            if g.is_none() {
                let s = UnixStream::connect(&self.path)
                    .map_err(|e| CallError::Unavailable(format!("{}: {e}", self.path.display())))?;
                s.set_read_timeout(Some(std::time::Duration::from_secs(10))).ok();
                let w = s.try_clone().map_err(|e| CallError::Unavailable(e.to_string()))?;
                *g = Some((BufReader::new(s), w));
            }
            let (r, w) = g.as_mut().unwrap();
            let mut answer = String::new();
            let ok = w.write_all(line.as_bytes()).is_ok() && matches!(r.read_line(&mut answer), Ok(n) if n > 0);
            if !ok {
                *g = None;
                if attempt == 0 {
                    continue;
                }
                return Err(CallError::Unavailable("identityd closed the connection".into()));
            }
            let v: Value = serde_json::from_str(&answer).map_err(|e| CallError::Unavailable(e.to_string()))?;
            if let Some(ok) = v.get("ok") {
                return Ok(ok.clone());
            }
            let e = v.get("error").cloned().unwrap_or(Value::Null);
            return Err(CallError::Refused {
                code: e.get("code").and_then(Value::as_str).unwrap_or("internal").to_string(),
                detail: e.get("detail").and_then(Value::as_str).map(str::to_string),
            });
        }
        Err(CallError::Unavailable("unreachable".into()))
    }
}
