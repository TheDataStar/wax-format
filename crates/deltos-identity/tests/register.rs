//! The privilege boundary's sign-in tests (docs/privilege-boundary.md §13),
//! against the register directly.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use deltos_identity::register::{Clock, Error, Grant, Kind, NewProfile, Register};
use deltos_identity::{policy, Ctx};

struct Fake(Arc<AtomicI64>);
impl Clock for Fake {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Box_ {
    reg: Register,
    t: Arc<AtomicI64>,
    _dir: tempfile::TempDir,
    admin_pw: String,
}

impl Box_ {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let t = Arc::new(AtomicI64::new(1_800_000_000));
        let reg = Register::open_with_clock(dir.path(), "home.deltos.lan", Box::new(Fake(Arc::clone(&t)))).unwrap();
        let (_, admin_pw) = reg.provision_first_admin("Admin", "admin").unwrap();
        Box_ { reg, t, _dir: dir, admin_pw }
    }
    fn advance(&self, s: i64) {
        self.t.fetch_add(s, Ordering::SeqCst);
    }
    fn admin(&self) -> String {
        self.reg.sign_in_password(&https(), "admin", &self.admin_pw).unwrap().session_token
    }
    fn teacher(&self, name: &str, class: &str) -> String {
        let a = self.admin();
        self.reg
            .create_profile(
                &https(),
                &a,
                &NewProfile {
                    display_name: name.into(),
                    kind: Kind::Passworded,
                    sign_in_name: Some(name.to_lowercase()),
                    password: Some("teacher password 1".into()),
                    class_or_group: Some(class.into()),
                    roles: vec![Grant { role: "teacher".into(), scope: Some(class.into()) }],
                },
            )
            .unwrap();
        self.reg.sign_in_password(&https(), &name.to_lowercase(), "teacher password 1").unwrap().session_token
    }
    fn student(&self, by: &str, name: &str, class: &str) -> String {
        self.reg
            .create_profile(
                &https(),
                by,
                &NewProfile {
                    display_name: name.into(),
                    kind: Kind::Passwordless,
                    sign_in_name: None,
                    password: None,
                    class_or_group: Some(class.into()),
                    roles: vec![Grant { role: "student".into(), scope: None }],
                },
            )
            .unwrap()
    }
}

fn http() -> Ctx {
    Ctx { peer: "10.42.0.20".into(), ..Ctx::default() }
}
fn https() -> Ctx {
    Ctx { https: true, peer: "10.42.0.21".into(), ..Ctx::default() }
}
fn kiosk() -> Ctx {
    Ctx { https: true, kiosk: true, peer: "127.0.0.1".into(), ..Ctx::default() }
}

#[test]
fn https_password_makes_a_secure_session_and_admin_holds_everything() {
    let b = Box_::new();
    let s = b.reg.sign_in_password(&https(), "admin", &b.admin_pw).unwrap();
    assert!(s.session.secure);
    assert!(s.session.holds("content.install", None));
    assert!(s.session.holds("some.app.permission", Some("x")), "admin holds permissions apps add later");
    assert_eq!(s.session.partition.len(), 16);
    let v = b.reg.session(&s.session_token).unwrap().unwrap();
    assert_eq!(v.csrf, s.session.csrf);
    assert!(b.reg.has_admin().unwrap());
    assert!(b.reg.provision_first_admin("Again", "admin2").is_err(), "provisioning works once");
}

#[test]
fn teacher_and_admin_passwords_are_refused_over_plain_http() {
    let b = Box_::new();
    b.teacher("Okafor", "5a");
    assert_eq!(b.reg.password_allowed(&http(), "okafor"), Err(Error::SecureRequired));
    assert_eq!(b.reg.password_allowed(&https(), "okafor"), Ok(()));
    assert_eq!(b.reg.sign_in_password(&http(), "admin", &b.admin_pw).unwrap_err(), Error::SecureRequired);
    assert_eq!(b.reg.sign_in_password(&http(), "okafor", "teacher password 1").unwrap_err(), Error::SecureRequired);
}

#[test]
fn a_passworded_person_without_secure_permissions_gets_a_plain_session_over_http() {
    let b = Box_::new();
    let a = b.admin();
    // a librarian-like role with no secure-only permission: password over HTTP is allowed
    b.reg
        .create_profile(
            &https(),
            &a,
            &NewProfile {
                display_name: "Reader".into(),
                kind: Kind::Passworded,
                sign_in_name: Some("reader".into()),
                password: Some("reader password".into()),
                class_or_group: None,
                roles: vec![Grant { role: "student".into(), scope: None }],
            },
        )
        .unwrap();
    let s = b.reg.sign_in_password(&http(), "reader", "reader password").unwrap();
    assert!(!s.session.secure, "made over plain HTTP: plain, forever");
}

#[test]
fn wrong_passwords_and_unknown_names_fail_the_same_way() {
    let b = Box_::new();
    assert_eq!(b.reg.sign_in_password(&https(), "admin", "nope nope nope").unwrap_err(), Error::SignInFailed);
    assert_eq!(b.reg.sign_in_password(&https(), "nobody", "nope nope nope").unwrap_err(), Error::SignInFailed);
}

#[test]
fn sign_in_is_rate_limited_per_person_then_recovers() {
    let b = Box_::new();
    for i in 0..policy::RATE_MAX_FAILURES {
        let c = Ctx { https: true, peer: format!("10.42.0.{}", 100 + i), ..Ctx::default() };
        assert_eq!(b.reg.sign_in_password(&c, "admin", "wrong wrong wrong").unwrap_err(), Error::SignInFailed);
    }
    // the right password from a fresh device is still refused: the person is limited
    let c = Ctx { https: true, peer: "10.42.0.250".into(), ..Ctx::default() };
    assert_eq!(b.reg.sign_in_password(&c, "admin", &b.admin_pw).unwrap_err(), Error::TooMany);
    b.advance(policy::RATE_WINDOW);
    assert!(b.reg.sign_in_password(&c, "admin", &b.admin_pw).is_ok());
}

#[test]
fn cards_sign_in_passwordless_people_to_plain_sessions_and_remember_the_device() {
    let b = Box_::new();
    let t = b.teacher("Okafor", "5a");
    let amara = b.student(&t, "Amara", "5a");
    // issuing a card is secure only
    assert_eq!(b.reg.issue_card(&http(), &t, &amara, None).unwrap_err(), Error::SecureRequired);
    let card = b.reg.issue_card(&https(), &t, &amara, None).unwrap();
    assert!(card.qr_text.starts_with("http://home.deltos.lan/#card="));
    // used over HTTPS it still makes only a plain session
    let s = b.reg.sign_in_card(&https(), &card.code.to_lowercase()).unwrap();
    assert!(!s.session.secure);
    assert_eq!(s.session.profile_id, amara);
    let device = s.device_token.clone().unwrap();
    // the remembered device starts a new session without the card
    let again = b.reg.sign_in_device(&http(), &device).unwrap();
    assert_eq!(again.session.profile_id, amara);
    assert!(!again.session.secure);
    // the teacher sees it and removes it: refused at once, and its session ends
    let devs = b.reg.list_devices(&https(), &t, &amara, None).unwrap();
    assert_eq!(devs.len(), 1);
    b.reg.remove_device(&https(), &t, &amara, None, &devs[0].device_id).unwrap();
    assert_eq!(b.reg.sign_in_device(&http(), &device).unwrap_err(), Error::NotSignedIn);
    assert!(b.reg.session(&again.session_token).unwrap().is_none());
}

#[test]
fn a_new_card_replaces_the_old_one_and_cards_expire() {
    let b = Box_::new();
    let t = b.teacher("Okafor", "5a");
    let amara = b.student(&t, "Amara", "5a");
    let old = b.reg.issue_card(&https(), &t, &amara, None).unwrap();
    let new = b.reg.issue_card(&https(), &t, &amara, None).unwrap();
    assert_eq!(b.reg.sign_in_card(&http(), &old.code).unwrap_err(), Error::SignInFailed);
    assert!(b.reg.sign_in_card(&http(), &new.code).is_ok());
    b.advance(policy::CARD_VALID);
    assert_eq!(b.reg.sign_in_card(&http(), &new.code).unwrap_err(), Error::SignInFailed);
}

#[test]
fn a_card_never_signs_in_a_passworded_person() {
    let b = Box_::new();
    let a = b.admin();
    let me = b.reg.session(&a).unwrap().unwrap().profile_id;
    assert!(matches!(b.reg.issue_card(&https(), &a, &me, None), Err(Error::Invalid(_))));
}

#[test]
fn teachers_manage_only_their_own_class() {
    let b = Box_::new();
    let ta = b.teacher("Okafor", "5a");
    let tb = b.teacher("Bello", "5b");
    let amara = b.student(&ta, "Amara", "5a");
    assert_eq!(b.reg.issue_card(&https(), &tb, &amara, None).unwrap_err(), Error::PermissionDenied);
    assert_eq!(b.reg.list_devices(&https(), &tb, &amara, None).unwrap_err(), Error::PermissionDenied);
    assert_eq!(b.reg.list_members(&https(), &tb, "5a").unwrap_err(), Error::PermissionDenied);
    // naming the wrong class in the path is refused even for the right teacher
    assert!(matches!(b.reg.issue_card(&https(), &ta, &amara, Some("5b")), Err(Error::Invalid(_))));
    assert!(b.reg.issue_card(&https(), &ta, &amara, Some("5a")).is_ok());
    assert_eq!(b.reg.list_members(&https(), &ta, "5a").unwrap().iter().filter(|m| m.kind == Kind::Passwordless).count(), 1);
    // a teacher may add a student to their own class only, and nothing else
    let other = NewProfile {
        display_name: "Kofi".into(),
        kind: Kind::Passwordless,
        sign_in_name: None,
        password: None,
        class_or_group: Some("5b".into()),
        roles: vec![Grant { role: "student".into(), scope: None }],
    };
    assert_eq!(b.reg.create_profile(&https(), &ta, &other).unwrap_err(), Error::PermissionDenied);
    let teacher = NewProfile {
        display_name: "X".into(),
        kind: Kind::Passworded,
        sign_in_name: Some("x".into()),
        password: Some("long enough password".into()),
        class_or_group: Some("5a".into()),
        roles: vec![Grant { role: "teacher".into(), scope: Some("5a".into()) }],
    };
    assert_eq!(b.reg.create_profile(&https(), &ta, &teacher).unwrap_err(), Error::PermissionDenied);
}

#[test]
fn tap_your_name_is_kiosk_only() {
    let b = Box_::new();
    let t = b.teacher("Okafor", "5a");
    let amara = b.student(&t, "Amara", "5a");
    assert_eq!(b.reg.tap_list(&http()).unwrap_err(), Error::PermissionDenied);
    assert_eq!(b.reg.tap_list(&https()).unwrap_err(), Error::PermissionDenied);
    let list = b.reg.tap_list(&kiosk()).unwrap();
    assert_eq!(list.len(), 1, "passworded people are not on the list");
    assert_eq!(b.reg.sign_in_tap(&https(), &amara).unwrap_err(), Error::PermissionDenied);
    let s = b.reg.sign_in_tap(&kiosk(), &amara).unwrap();
    assert!(s.session.kiosk && !s.session.secure);
    let me = b.reg.session(&t).unwrap().unwrap().profile_id;
    assert_eq!(b.reg.sign_in_tap(&kiosk(), &me).unwrap_err(), Error::SignInFailed);
}

#[test]
fn switching_person_on_one_browser_ends_the_first_and_partitions_differ() {
    let b = Box_::new();
    let t = b.teacher("Okafor", "5a");
    let amara = b.student(&t, "Amara", "5a");
    let kofi = b.student(&t, "Kofi", "5a");
    let first = b.reg.sign_in_tap(&kiosk(), &amara).unwrap();
    let ctx = Ctx { browser: Some(first.browser_token.clone()), session: Some(first.session_token.clone()), ..kiosk() };
    let second = b.reg.sign_in_tap(&ctx, &kofi).unwrap();
    assert_eq!(second.browser_token, first.browser_token, "same browser");
    assert!(b.reg.session(&first.session_token).unwrap().is_none(), "A's session ended before B's began");
    assert_ne!(first.session.partition, second.session.partition);
    assert!(b.reg.session_for_partition(&first.session.partition).unwrap().is_none(), "A's pack hostnames shut");
    assert_eq!(b.reg.session_for_partition(&second.session.partition).unwrap().unwrap().profile_id, kofi);
    // A again on the same browser: the same partition as before (their own storage)
    let ctx = Ctx { browser: Some(first.browser_token.clone()), session: Some(second.session_token.clone()), ..kiosk() };
    let third = b.reg.sign_in_tap(&ctx, &amara).unwrap();
    assert_eq!(third.session.partition, first.session.partition);
    // a browser cookie the box never issued gets a new browser, so a new partition
    let ctx = Ctx { browser: Some("ab".repeat(32)), ..kiosk() };
    let fourth = b.reg.sign_in_tap(&ctx, &amara).unwrap();
    assert_ne!(fourth.browser_token, "ab".repeat(32));
    assert_ne!(fourth.session.partition, first.session.partition);
}

#[test]
fn sign_out_shuts_pack_hostnames_and_everywhere_ends_all() {
    let b = Box_::new();
    let a1 = b.reg.sign_in_password(&https(), "admin", &b.admin_pw).unwrap();
    let a2 = b.reg.sign_in_password(&https(), "admin", &b.admin_pw).unwrap();
    assert_ne!(a1.session.partition, a2.session.partition, "two browsers");
    b.reg.sign_out(&a1.session_token, None).unwrap();
    assert!(b.reg.session_for_partition(&a1.session.partition).unwrap().is_none());
    assert!(b.reg.session(&a2.session_token).unwrap().is_some());
    b.reg.sign_out_everywhere(&a2.session_token).unwrap();
    assert!(b.reg.session(&a2.session_token).unwrap().is_none());
}

#[test]
fn sessions_end_when_idle_or_too_old() {
    let b = Box_::new();
    let t = b.teacher("Okafor", "5a");
    let amara = b.student(&t, "Amara", "5a");
    let k = b.reg.sign_in_tap(&kiosk(), &amara).unwrap();
    b.advance(policy::KIOSK_IDLE - 1);
    assert!(b.reg.session(&k.session_token).unwrap().is_some());
    b.advance(policy::KIOSK_IDLE);
    assert!(b.reg.session(&k.session_token).unwrap().is_none(), "idle on the kiosk");

    let s = b.reg.sign_in_password(&https(), "admin", &b.admin_pw).unwrap();
    let steps = policy::PERSONAL_SECURE_ABSOLUTE / (policy::PERSONAL_SECURE_IDLE - 60) + 1;
    for _ in 0..steps {
        b.advance(policy::PERSONAL_SECURE_IDLE - 60);
        b.reg.session(&s.session_token).unwrap();
    }
    assert!(b.reg.session(&s.session_token).unwrap().is_none(), "absolute limit, even when kept busy");
}

#[test]
fn tickets_are_single_use_bound_and_short_lived() {
    let b = Box_::new();
    let a = b.admin();
    let d1 = "1".repeat(64);
    let d2 = "2".repeat(64);
    assert_eq!(
        b.reg.issue_ticket(&https(), &a, "wrong password!", "remove_pack", &d1).unwrap_err(),
        Error::ConfirmationRequired
    );
    let t = b.reg.issue_ticket(&https(), &a, &b.admin_pw, "remove_pack", &d1).unwrap();
    assert!(b.reg.redeem_ticket(&a, &t, "remove_pack", &d1).is_ok());
    assert!(b.reg.redeem_ticket(&a, &t, "remove_pack", &d1).is_err(), "used twice");
    let t = b.reg.issue_ticket(&https(), &a, &b.admin_pw, "remove_pack", &d1).unwrap();
    assert!(b.reg.redeem_ticket(&a, &t, "remove_pack", &d2).is_err(), "other arguments");
    let t = b.reg.issue_ticket(&https(), &a, &b.admin_pw, "remove_pack", &d1).unwrap();
    assert!(b.reg.redeem_ticket(&a, &t, "configure_ap", &d1).is_err(), "other action");
    let t = b.reg.issue_ticket(&https(), &a, &b.admin_pw, "remove_pack", &d1).unwrap();
    let other = b.admin();
    assert!(b.reg.redeem_ticket(&other, &t, "remove_pack", &d1).is_err(), "other session");
    let t = b.reg.issue_ticket(&https(), &a, &b.admin_pw, "remove_pack", &d1).unwrap();
    b.advance(policy::TICKET_VALID);
    assert!(b.reg.redeem_ticket(&a, &t, "remove_pack", &d1).is_err(), "after a minute");
    // over plain HTTP no ticket is issued
    assert_eq!(b.reg.issue_ticket(&http(), &a, &b.admin_pw, "remove_pack", &d1).unwrap_err(), Error::SecureRequired);
}

#[test]
fn a_passwordless_person_can_never_reconfirm() {
    let b = Box_::new();
    let t = b.teacher("Okafor", "5a");
    let amara = b.student(&t, "Amara", "5a");
    let card = b.reg.issue_card(&https(), &t, &amara, None).unwrap();
    let s = b.reg.sign_in_card(&https(), &card.code).unwrap();
    // even over HTTPS, a card session is plain, so no ticket
    assert_eq!(
        b.reg.issue_ticket(&https(), &s.session_token, "", "remove_pack", &"1".repeat(64)).unwrap_err(),
        Error::SecureRequired
    );
}

#[test]
fn a_password_change_ends_every_session() {
    let b = Box_::new();
    let a1 = b.admin();
    let a2 = b.admin();
    b.reg.change_password(&https(), &a1, &b.admin_pw, "a brand new password").unwrap();
    assert!(b.reg.session(&a1).unwrap().is_none() && b.reg.session(&a2).unwrap().is_none());
    assert!(b.reg.sign_in_password(&https(), "admin", "a brand new password").is_ok());
}

#[test]
fn the_database_refuses_bad_shapes_and_newer_schemas() {
    let dir = tempfile::tempdir().unwrap();
    {
        let reg = Register::open(dir.path(), "home.deltos.lan").unwrap();
        let (_, pw) = reg.provision_first_admin("Admin", "admin").unwrap();
        let a = reg.sign_in_password(&https(), "admin", &pw).unwrap().session_token;
        let bad = NewProfile {
            display_name: "Pwless".into(),
            kind: Kind::Passwordless,
            sign_in_name: Some("pw".into()),
            password: None,
            class_or_group: None,
            roles: vec![],
        };
        assert!(matches!(reg.create_profile(&https(), &a, &bad), Err(Error::Invalid(_))));
        let short = NewProfile { kind: Kind::Passworded, password: Some("short".into()), ..bad };
        assert!(matches!(reg.create_profile(&https(), &a, &short), Err(Error::Invalid(_))));
    }
    let c = rusqlite::Connection::open(dir.path().join("identity.sqlite")).unwrap();
    // the table-level CHECK of contract §5.1: a passworded row without a hash cannot exist
    assert!(c
        .execute("INSERT INTO profiles VALUES ('x','X','passworded','x',NULL,NULL,0)", [])
        .is_err());
    c.execute("UPDATE meta SET value = '99' WHERE key = 'schema_version'", []).unwrap();
    drop(c);
    assert!(Register::open(dir.path(), "home.deltos.lan").is_err());
}

#[cfg(unix)]
#[test]
fn files_are_owner_only_and_no_secret_is_stored_in_the_clear() {
    use std::os::unix::fs::PermissionsExt;
    let b = Box_::new();
    let t = b.teacher("Okafor", "5a");
    let amara = b.student(&t, "Amara", "5a");
    let card = b.reg.issue_card(&https(), &t, &amara, None).unwrap();
    let s = b.reg.sign_in_card(&http(), &card.code).unwrap();
    for f in ["identity.sqlite", "card-key"] {
        let m = std::fs::metadata(b._dir.path().join(f)).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "{f}");
    }
    let mut all = Vec::new();
    for f in std::fs::read_dir(b._dir.path()).unwrap() {
        all.extend(std::fs::read(f.unwrap().path()).unwrap());
    }
    let hay = String::from_utf8_lossy(&all);
    for secret in [&s.session_token, s.device_token.as_ref().unwrap(), &s.browser_token, &card.code.replace('-', ""), &b.admin_pw] {
        assert!(!hay.contains(secret.as_str()), "a secret is stored in the clear");
    }
}
