//! The box API's side of the socket: one kept connection, reopened if
//! shelld restarted. For tests and tools; `deltd` itself uses
//! `deltlib::admin::ShelldLink` (deltd API §6.8), the same wire format.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::Value;

type Conn = Option<(BufReader<UnixStream>, UnixStream)>;

pub struct Client {
    path: PathBuf,
    conn: Mutex<Conn>,
}

/// `Err` is the error object `{code, detail}`, or `unavailable` when shelld
/// cannot be reached.
impl Client {
    pub fn new(path: impl Into<PathBuf>) -> Client {
        Client { path: path.into(), conn: Mutex::new(None) }
    }

    pub fn call(&self, req: &Value) -> Result<Value, Value> {
        let unavailable = |d: String| serde_json::json!({ "code": "unavailable", "detail": d });
        let mut line = req.to_string();
        line.push('\n');
        let mut g = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        for attempt in 0..2 {
            if g.is_none() {
                let s = UnixStream::connect(&self.path).map_err(|e| unavailable(format!("{}: {e}", self.path.display())))?;
                let w = s.try_clone().map_err(|e| unavailable(e.to_string()))?;
                *g = Some((BufReader::new(s), w));
            }
            let (r, w) = g.as_mut().unwrap();
            let mut answer = String::new();
            if w.write_all(line.as_bytes()).is_err() || !matches!(r.read_line(&mut answer), Ok(n) if n > 0) {
                *g = None;
                if attempt == 0 {
                    continue;
                }
                return Err(unavailable("shelld closed the connection".into()));
            }
            let v: Value = serde_json::from_str(&answer).map_err(|e| unavailable(e.to_string()))?;
            if let Some(ok) = v.get("ok") {
                return Ok(ok.clone());
            }
            return Err(v.get("error").cloned().unwrap_or(Value::Null));
        }
        unreachable!()
    }
}
