//! The socket: who may connect, and what each caller may ask.

use std::os::unix::net::UnixListener;
use std::sync::Arc;

use deltos_identity::protocol::{self, answer, Caller, Callers};
use deltos_identity::{CallError, Client, Register};
use serde_json::json;

fn me() -> u32 {
    unsafe { libc::getuid() }
}

fn start(callers: Callers) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let reg = Arc::new(Register::open(&dir.path().join("reg"), "home.deltos.lan").unwrap());
    let (_, pw) = reg.provision_first_admin("Admin", "admin").unwrap();
    let sock = dir.path().join("identityd.sock");
    let l = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || protocol::serve(l, reg, callers));
    (dir, sock, pw)
}

#[test]
fn deltd_signs_in_over_the_socket() {
    let (_d, sock, pw) = start(Callers { deltd: vec![me()], shelld: vec![] });
    let c = Client::new(&sock);
    let ctx = json!({ "https": true, "peer": "10.42.0.9" });
    let s = c
        .call(&json!({ "call": "sign_in_password", "ctx": ctx, "sign_in_name": "admin", "password": pw }))
        .unwrap();
    let token = s["session_token"].as_str().unwrap();
    let v = c.call(&json!({ "call": "session", "token": token })).unwrap();
    assert_eq!(v["secure"], true);
    let e = c.call(&json!({ "call": "sign_in_password", "ctx": ctx, "sign_in_name": "admin", "password": "x" }));
    assert!(matches!(e, Err(CallError::Refused { code, .. }) if code == "sign_in_failed"));
    // deltd may not redeem tickets: that is shelld's call alone
    let e = c.call(&json!({ "call": "redeem_ticket", "token": token, "ticket": "a", "action": "x", "args_digest": "y" }));
    assert!(matches!(e, Err(CallError::Refused { code, .. }) if code == "permission_denied"));
}

#[test]
fn a_caller_that_is_not_deltd_or_shelld_is_disconnected() {
    let (_d, sock, _) = start(Callers { deltd: vec![me().wrapping_add(1)], shelld: vec![] });
    let c = Client::new(&sock);
    assert!(matches!(c.call(&json!({ "call": "session", "token": "x" })), Err(CallError::Unavailable(_))));
}

#[test]
fn shelld_may_only_check_sessions_and_redeem_tickets() {
    let dir = tempfile::tempdir().unwrap();
    let reg = Register::open(dir.path(), "home.deltos.lan").unwrap();
    for call in ["sign_in_password", "sign_in_card", "create_profile", "issue_card", "issue_ticket", "set_password"] {
        let v = answer(&reg, Caller::Shelld, &json!({ "call": call }).to_string());
        assert_eq!(v["error"]["code"], "permission_denied", "{call}");
    }
    for call in ["session", "has_permission", "redeem_ticket"] {
        let v = answer(&reg, Caller::Shelld, &json!({ "call": call, "token": "x", "permission": "p", "ticket": "t", "action": "a", "args_digest": "d" }).to_string());
        assert_ne!(v["error"]["code"], "permission_denied", "{call}");
    }
    let v = answer(&reg, Caller::Deltd, "not json");
    assert_eq!(v["error"]["code"], "invalid_request");
}

#[test]
fn shelld_s_permission_check_wants_a_secure_session_for_secure_only_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let reg = Register::open(dir.path(), "home.deltos.lan").unwrap();
    // a librarian-like passworded person with a plain session from HTTP
    let (_, pw) = reg.provision_first_admin("Admin", "admin").unwrap();
    let secure = reg
        .sign_in_password(&deltos_identity::Ctx { https: true, ..Default::default() }, "admin", &pw)
        .unwrap()
        .session_token;
    let q = |t: &str, p: &str| answer(&reg, Caller::Shelld, &json!({ "call": "has_permission", "token": t, "permission": p }).to_string());
    assert_eq!(q(&secure, "content.install")["ok"], true);
    assert_eq!(q("0".repeat(64).as_str(), "content.install")["ok"], false);
}
