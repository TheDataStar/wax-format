//! The socket, the callers, and the one worker that changes the box.
//!
//! **Wire format** (the same as `identityd`'s): one JSON object per line
//! each way over a Unix stream socket. A request is `{"call": ...}`; the
//! answer is `{"ok": <value>}` or `{"error": {"code", "detail"}}`, codes from
//! deltd API §7. The calls (`docs/shelld.md` §3):
//!
//! * `describe` `{action, args}`: the permission, whether it asks again, and
//!   the argument digest a confirmation ticket must be bound to. Changes nothing.
//! * `act` `{action, args, session, over_https, ticket?}`: check and perform
//!   one action. A short one answers with its result; a long one answers
//!   `{"job": id}` at once.
//! * `job` `{job, session, over_https}`: a job's state, for the person who started it.
//! * `list` `{listing, session, over_https}`: `addons`, `addon_sources`,
//!   `drives`, `leases`, `action_log`, each behind its permission.
//!
//! **Callers** (rules 1 and 2): the socket is `0660` in a folder whose group
//! is `deltos-shellc`, and on every connection the caller's user id is read
//! from the kernel. Only `deltos-deltd` is served; anyone else is
//! disconnected before a byte is read.
//!
//! **One worker.** Every authorized action runs on a single thread, one at a
//! time and in the order checked, so two actions never change the same
//! register at once, and installs run one at a time (Track F §4).

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use deltos_identity::protocol::peer_uid;
use deltos_identity::Client;
use deltos_net::actions::{Caller, IdentityCheck, Refusal};
use serde::Serialize;
use serde_json::{json, Value};

use crate::action::{Action, BUILT, NOT_BUILT};
use crate::addons::{self, Folders, Sources};
use crate::check::Ask;
use crate::drives::DrivesDir;
use crate::log::{now, ActionLog, Entry};

/// Largest request line (a configure_ap or backup request is well under 1 KiB).
pub const MAX_LINE: usize = 64 * 1024;
/// Finished jobs kept for their starters to read.
pub const KEEP_JOBS: usize = 64;

/// The network registry and the settings folder it renders into.
pub struct NetPaths {
    pub registry: PathBuf,
    pub settings: deltos_net::Layout,
}

/// Everything shelld acts on.
pub struct Config {
    pub identityd: Client,
    pub log: ActionLog,
    /// The `deltos-deltd` user's id (tests: the test's own).
    pub deltd_uids: Vec<u32>,
    pub net: NetPaths,
    pub backup: deltos_backup::Layout,
    pub addons: addons::Layout,
    /// Extra folders bundles are listed from, by name (`--addon-source`).
    pub addon_folders: Vec<(String, PathBuf)>,
    pub drives: DrivesDir,
    pub trust: addons::Trust,
}

impl Config {
    fn sources(&self) -> Folders {
        let mut o = self.addon_folders.clone();
        o.extend(self.drives.origins());
        Folders(o)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub action: String,
    #[serde(skip)]
    pub by: String,
    /// `queued`, then the action's own states (contract §7.2 for installs),
    /// ending in `done` or a failure (`failed_signature`, `failed`).
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
}

/// An action that passed its family's checks.
enum Authz {
    Net(deltos_net::Authorized),
    Backup(deltos_backup::Authorized),
    Addon(addons::Authorized),
}

struct Work {
    authz: Authz,
    by: String,
    name: &'static str,
    summary: String,
    job: Option<String>,
    reply: Option<Sender<Result<Value, Failure>>>,
}

#[derive(Debug, Clone)]
struct Failure {
    code: String,
    detail: String,
}

impl Failure {
    fn new(code: &str, detail: impl Into<String>) -> Self {
        Failure { code: code.into(), detail: detail.into() }
    }
    fn to_json(&self) -> Value {
        json!({ "error": { "code": self.code, "detail": self.detail } })
    }
}

pub struct Shell {
    cfg: Arc<Config>,
    jobs: Arc<Mutex<BTreeMap<String, Job>>>,
    finished: Arc<Mutex<VecDeque<String>>>,
    work: Mutex<Sender<Work>>,
}

fn refusal(r: &Refusal, ask: &Ask) -> Failure {
    match r {
        Refusal::NotSignedIn if ask.was_unavailable() => Failure::new("sign_in_unavailable", "the sign-in service cannot be reached"),
        Refusal::NotSignedIn => Failure::new("not_signed_in", "no live session"),
        Refusal::SecureConnectionRequired => {
            Failure::new("secure_connection_required", "changing the box needs HTTPS and a session made over HTTPS")
        }
        Refusal::PermissionDenied => Failure::new("permission_denied", "the person does not hold the permission this needs"),
        Refusal::ConfirmationRequired => Failure::new("confirmation_required", "this needs the password again"),
        Refusal::Invalid(d) => Failure::new("invalid_request", d.trim_start_matches("invalid_request: ")),
    }
}

fn random_id() -> String {
    let mut b = [0u8; 12];
    getrandom::getrandom(&mut b).expect("the operating system's random source");
    crate::addons::hex(&b)
}

impl Shell {
    /// Starts the worker thread. The restore's services move to it: it is
    /// the only thread that uses them.
    /// `services` are what a restore stops and starts around the swap.
    pub fn new(cfg: Config, mut services: Box<dyn deltos_backup::Services + Send>) -> Arc<Shell> {
        let (tx, rx) = channel::<Work>();
        let shell = Arc::new(Shell { cfg: Arc::new(cfg), jobs: Arc::default(), finished: Arc::default(), work: Mutex::new(tx) });
        let cfg = Arc::clone(&shell.cfg);
        let jobs = Arc::clone(&shell.jobs);
        let finished = Arc::clone(&shell.finished);
        std::thread::Builder::new()
            .name("shelld-worker".into())
            .spawn(move || {
                for w in rx {
                    run(&cfg, &jobs, &finished, services.as_mut(), w);
                }
            })
            .expect("a thread");
        shell
    }

    /// Answer one request line.
    pub fn answer(&self, line: &str) -> Value {
        let req: Value = match serde_json::from_str(line) {
            Ok(v @ Value::Object(_)) => v,
            _ => return Failure::new("invalid_request", "not a JSON object").to_json(),
        };
        let r = match req.get("call").and_then(Value::as_str).unwrap_or("") {
            "describe" => self.describe(&req),
            "act" => self.act(&req),
            "job" => self.job(&req),
            "list" => self.list(&req),
            c => Err(Failure::new("invalid_request", format!("no call {c:?}"))),
        };
        match r {
            Ok(v) => json!({ "ok": v }),
            Err(f) => f.to_json(),
        }
    }

    fn parse(req: &Value) -> Result<Action, Failure> {
        let name = req.get("action").and_then(Value::as_str).ok_or_else(|| Failure::new("invalid_request", "action"))?;
        let args = req.get("args").cloned().unwrap_or_else(|| json!({}));
        Action::parse(name, &args).map_err(|e| Failure::new("invalid_request", e))
    }

    fn describe(&self, req: &Value) -> Result<Value, Failure> {
        let a = Self::parse(req)?;
        Ok(json!({
            "action": a.name(),
            "permission": a.permission(),
            "needs_reconfirm": a.needs_reconfirm(),
            "args_digest": a.args_digest(),
            "long": a.is_long(),
        }))
    }

    fn caller(req: &Value) -> Result<(String, bool, Option<String>), Failure> {
        let s = req.get("session").and_then(Value::as_str).ok_or_else(|| Failure::new("not_signed_in", "no session"))?;
        let https = req.get("over_https").and_then(Value::as_bool).unwrap_or(false);
        let t = req.get("ticket").and_then(Value::as_str).map(str::to_string);
        Ok((s.to_string(), https, t))
    }

    fn log(&self, by: &str, action: &str, args: &str, result: &str, detail: Option<String>, job: Option<String>) -> Result<(), Failure> {
        self.cfg
            .log
            .write(&Entry { at: now(), by: by.into(), action: action.into(), args: args.into(), result: result.into(), detail, job })
            .map_err(|e| Failure::new("internal", format!("the action log cannot be written: {e}")))
    }

    fn act(&self, req: &Value) -> Result<Value, Failure> {
        let raw_name: String = req.get("action").and_then(Value::as_str).unwrap_or("").chars().take(64).collect();
        let a = match Self::parse(req) {
            Ok(a) => a,
            Err(f) => {
                self.log("unknown", &raw_name, "", &f.code, Some(f.detail.clone()), None)?;
                return Err(f);
            }
        };
        let (session, over_https, ticket) = Self::caller(req)?;
        let caller = Caller { session_ref: &session, over_https, confirmation_ticket: ticket.as_deref() };
        let ask = Ask::new(&self.cfg.identityd);
        let (name, summary, long) = (a.name(), a.summary(), a.is_long());
        let authz = match a {
            Action::Net(n) => deltos_net::actions::authorize(n, &caller, &ask).map(Authz::Net),
            Action::Backup(b) => deltos_backup::actions::authorize(b, &caller, &ask).map(Authz::Backup),
            Action::Addon(x) => addons::authorize(x, &caller, &ask).map(Authz::Addon),
        };
        let authz = match authz {
            Ok(z) => z,
            Err(r) => {
                let f = refusal(&r, &ask);
                let by = ask.session(&session).map(|(p, _)| p).unwrap_or_else(|| "unknown".into());
                self.log(&by, name, &summary, &f.code, Some(f.detail.clone()), None)?;
                return Err(f);
            }
        };
        let by = match &authz {
            Authz::Net(z) => z.profile_id().to_string(),
            Authz::Backup(z) => z.by().to_string(),
            Authz::Addon(z) => z.by().to_string(),
        };
        if long {
            let id = random_id();
            self.jobs.lock().unwrap().insert(
                id.clone(),
                Job { id: id.clone(), action: name.into(), by: by.clone(), state: "queued".into(), result: None, error: None, started_at: now(), finished_at: None },
            );
            self.log(&by, name, &summary, "started", None, Some(id.clone()))?;
            self.send(Work { authz, by, name, summary, job: Some(id.clone()), reply: None })?;
            return Ok(json!({ "job": id }));
        }
        let (tx, rx) = channel();
        self.send(Work { authz, by, name, summary, job: None, reply: Some(tx) })?;
        rx.recv().map_err(|_| Failure::new("internal", "the worker stopped"))?
    }

    fn send(&self, w: Work) -> Result<(), Failure> {
        self.work.lock().unwrap().send(w).map_err(|_| Failure::new("internal", "the worker stopped"))
    }

    fn job(&self, req: &Value) -> Result<Value, Failure> {
        let (session, over_https, _) = Self::caller(req)?;
        let ask = Ask::new(&self.cfg.identityd);
        let (profile, secure) = ask.session(&session).ok_or_else(|| refusal(&Refusal::NotSignedIn, &ask))?;
        if !over_https || !secure {
            return Err(refusal(&Refusal::SecureConnectionRequired, &ask));
        }
        let id = req.get("job").and_then(Value::as_str).unwrap_or("");
        let jobs = self.jobs.lock().unwrap();
        match jobs.get(id) {
            // another person's job is not there, as far as they can tell
            Some(j) if j.by == profile => Ok(serde_json::to_value(j).unwrap()),
            _ => Err(Failure::new("invalid_request", "no such job")),
        }
    }

    fn reader(&self, req: &Value, permission: &str) -> Result<(), Failure> {
        let (session, over_https, _) = Self::caller(req)?;
        let ask = Ask::new(&self.cfg.identityd);
        let (profile, secure) = ask.session(&session).ok_or_else(|| refusal(&Refusal::NotSignedIn, &ask))?;
        if !over_https || !secure {
            return Err(refusal(&Refusal::SecureConnectionRequired, &ask));
        }
        if !ask.has_permission(&profile, permission) {
            return Err(refusal(&Refusal::PermissionDenied, &ask));
        }
        Ok(())
    }

    fn list(&self, req: &Value) -> Result<Value, Failure> {
        let internal = |e: addons::Error| Failure::new(e.code(), e.to_string());
        match req.get("listing").and_then(Value::as_str).unwrap_or("") {
            "actions" => Ok(json!({ "built": BUILT, "not_built": NOT_BUILT })),
            "addons" => {
                self.reader(req, addons::MANAGE)?;
                let reg = addons::Register::load(&self.cfg.addons).map_err(internal)?;
                let ms = addons::installed_manifests(&self.cfg.addons, &reg, None).map_err(internal)?;
                let apps: Vec<Value> = ms
                    .iter()
                    .map(|m| {
                        let r = reg.apps.get(&m.id);
                        json!({
                            "id": m.id, "name": m.name, "summary": m.summary, "kind": m.kind,
                            "version": m.version,
                            "previous": r.and_then(|r| r.previous.clone()),
                            "wanted_running": r.map(|r| r.wanted_running),
                        })
                    })
                    .collect();
                Ok(json!({ "apps": apps }))
            }
            "addon_sources" => {
                self.reader(req, addons::MANAGE)?;
                Ok(json!({ "bundles": self.cfg.sources().list() }))
            }
            "drives" => {
                self.reader(req, deltos_backup::actions::PERMISSION)?;
                Ok(json!({ "drives": self.cfg.drives.list() }))
            }
            "leases" => {
                let (session, over_https, _) = Self::caller(req)?;
                let ask = Ask::new(&self.cfg.identityd);
                let caller = Caller { session_ref: &session, over_https, confirmation_ticket: None };
                let l = deltos_net::actions::list_leases(&caller, &ask, &self.cfg.net.settings, now())
                    .map_err(|r| refusal(&r, &ask))?;
                Ok(json!({ "leases": l.iter().map(|x| json!({
                    "mac": x.mac, "address": x.address.to_string(), "device_name": x.device_name, "expires_unix": x.expires_unix,
                })).collect::<Vec<_>>() }))
            }
            "action_log" => {
                self.reader(req, "admin.console")?;
                let t = self.cfg.log.tail(200).map_err(|e| Failure::new("internal", e.to_string()))?;
                Ok(json!({ "entries": t }))
            }
            l => Err(Failure::new("invalid_request", format!("no listing {l:?}"))),
        }
    }
}

fn set_job(jobs: &Mutex<BTreeMap<String, Job>>, id: &Option<String>, f: impl FnOnce(&mut Job)) {
    if let Some(id) = id {
        if let Some(j) = jobs.lock().unwrap().get_mut(id) {
            f(j);
        }
    }
}

fn run(cfg: &Config, jobs: &Mutex<BTreeMap<String, Job>>, finished: &Mutex<VecDeque<String>>, services: &mut dyn deltos_backup::Services, w: Work) {
    set_job(jobs, &w.job, |j| j.state = "running".into());
    let r: Result<Value, Failure> = match w.authz {
        Authz::Net(z) => deltos_net::Registry::open(&cfg.net.registry)
            .and_then(|mut reg| deltos_net::apply(&mut reg, z, &cfg.net.settings))
            .map(|_| json!({ "done": true }))
            .map_err(|e| Failure::new("invalid_request", e.to_string())),
        Authz::Backup(z) => deltos_backup::actions::perform(&cfg.backup, z, &cfg.drives, services)
            .map(|o| match o {
                deltos_backup::actions::Outcome::Created(c) => json!({ "created": c }),
                deltos_backup::actions::Outcome::Reviewed(r) => json!({ "review": r }),
                deltos_backup::actions::Outcome::Restored(a) => json!({ "restored": a }),
            })
            .map_err(|e| Failure::new(e.code(), e.to_string())),
        Authz::Addon(z) => {
            let mut progress = |s: &str| set_job(jobs, &w.job, |j| j.state = s.into());
            addons::perform(&cfg.addons, z, &cfg.sources(), &cfg.trust, &addons::Measured(&cfg.addons), &mut progress)
                .map(|o| serde_json::to_value(o).unwrap())
                .map_err(|e| Failure::new(e.code(), e.to_string()))
        }
    };
    let (result, detail) = match &r {
        Ok(_) => ("ok".to_string(), None),
        Err(f) => (f.code.clone(), Some(f.detail.clone())),
    };
    let logged = cfg.log.write(&Entry { at: now(), by: w.by, action: w.name.into(), args: w.summary, result, detail, job: w.job.clone() });
    let r = match logged {
        Ok(()) => r,
        Err(e) => Err(Failure::new("internal", format!("done, but the action log cannot be written: {e}"))),
    };
    if let Some(id) = &w.job {
        set_job(jobs, &w.job, |j| {
            j.finished_at = Some(now());
            match &r {
                Ok(v) => {
                    j.state = "done".into();
                    j.result = Some(v.clone());
                }
                Err(f) => {
                    j.state = if f.code == "signature_invalid" { "failed_signature".into() } else { "failed".into() };
                    j.error = Some(json!({ "code": f.code, "detail": f.detail }));
                }
            }
        });
        let mut fin = finished.lock().unwrap();
        fin.push_back(id.clone());
        while fin.len() > KEEP_JOBS {
            if let Some(old) = fin.pop_front() {
                jobs.lock().unwrap().remove(&old);
            }
        }
    }
    if let Some(tx) = w.reply {
        let _ = tx.send(r);
    }
}

/// Serve the socket forever: a thread per connection, only from deltd.
pub fn serve(listener: UnixListener, shell: Arc<Shell>) -> std::io::Result<()> {
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        match peer_uid(&conn) {
            Ok(u) if shell.cfg.deltd_uids.contains(&u) => {}
            _ => {
                eprintln!("deltos-shelld: refused a connection from a caller that is not deltd");
                continue;
            }
        }
        let shell = Arc::clone(&shell);
        std::thread::spawn(move || {
            let _ = serve_conn(conn, &shell);
        });
    }
    Ok(())
}

fn serve_conn(conn: UnixStream, shell: &Shell) -> std::io::Result<()> {
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
            let mut v = Failure::new("invalid_request", "request too long").to_json().to_string();
            v.push('\n');
            w.write_all(v.as_bytes())?;
            return Ok(());
        }
        let mut v = shell.answer(line.trim_end()).to_string();
        v.push('\n');
        w.write_all(v.as_bytes())?;
    }
}
