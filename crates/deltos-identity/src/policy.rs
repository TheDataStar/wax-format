//! The numbers `identityd` runs on, in one place.
//!
//! Every value here was open in contract §13.2 ("how long a session lasts …
//! the sign-in card's code length and validity") or deltd API §10 (rate-limit
//! numbers), and was chosen on 2026-11-21 under Commander's delegation of
//! routine decisions. The reasoning for each is in `docs/identityd.md` §5.
//! The rate limits and the password-hash cost are **starting values**, to be
//! measured on the minimum spec (2 GB aarch64) and adjusted, never treated as
//! measured.

/// Seconds.
pub type Secs = i64;

const MIN: Secs = 60;
const HOUR: Secs = 60 * MIN;
const DAY: Secs = 24 * HOUR;

/// A session on the box's own shared screen (the kiosk), plain or secure:
/// ends after 10 idle minutes, and after 8 hours (a school day) regardless.
pub const KIOSK_IDLE: Secs = 10 * MIN;
pub const KIOSK_ABSOLUTE: Secs = 8 * HOUR;

/// A plain session on a person's own device. A remembered device starts a
/// new one without the card, so a short absolute limit costs a learner
/// nothing.
pub const PERSONAL_PLAIN_IDLE: Secs = DAY;
pub const PERSONAL_PLAIN_ABSOLUTE: Secs = 7 * DAY;

/// A secure session on a person's own device (teachers and admins). Never
/// longer than a plain one (privilege boundary §4.2).
pub const PERSONAL_SECURE_IDLE: Secs = 30 * MIN;
pub const PERSONAL_SECURE_ABSOLUTE: Secs = 12 * HOUR;

/// A remembered device (`delt_device`) ends after 90 days unused, and after
/// a year regardless.
pub const DEVICE_UNUSED: Secs = 90 * DAY;
pub const DEVICE_ABSOLUTE: Secs = 365 * DAY;

/// A sign-in card works for 14 days from issue, for any number of the
/// person's devices, until it expires or a new card replaces it.
pub const CARD_VALID: Secs = 14 * DAY;
/// Characters in a card's code, from the 32-character Crockford alphabet:
/// 50 bits, shown as `XXXXX-XXXXX`.
pub const CARD_CODE_LEN: usize = 10;

/// A re-confirmation ticket lives one minute at most (privilege boundary §6).
pub const TICKET_VALID: Secs = 60;

/// Failed sign-in, card and re-confirm attempts allowed per device address
/// and per person within the window, before further attempts are refused
/// until the window passes. Starting values (deltd API §9, F15).
pub const RATE_WINDOW: Secs = 10 * MIN;
pub const RATE_MAX_FAILURES: u32 = 10;

/// Passwords set for passworded people.
pub const PASSWORD_MIN_CHARS: usize = 10;
pub const PASSWORD_MAX_BYTES: usize = 256;

/// Argon2id cost: OWASP's minimum recommendation (19 MiB, 2 passes, 1 lane),
/// chosen to fit 40 phones' sign-ins on a 2 GB box. Starting value.
pub const ARGON2_M_KIB: u32 = 19 * 1024;
pub const ARGON2_T: u32 = 2;
pub const ARGON2_P: u32 = 1;

/// `last_active` is written at most this often per session, so a busy
/// classroom does not wear the SD card with a write per request.
pub const TOUCH_EVERY: Secs = MIN;

/// Permissions usable only over HTTPS with a session made over HTTPS
/// (contract §4.2, privilege boundary rule 9). Holding any of them also
/// makes a person's password HTTPS-only (contract §5.1).
pub const SECURE_ONLY: &[&str] = &[
    "content.install",
    // installing software that runs on the box (contract §4.2, 2026-11-22)
    "apps.manage",
    "network.configure",
    "service.restart",
    "fleet.manage",
    "backup.manage",
    "system.update",
    "progress.view",
    "admin.console",
];

pub fn is_secure_only(permission: &str) -> bool {
    SECURE_ONLY.contains(&permission)
}

/// The idle and absolute limits for a new session.
pub fn session_limits(kiosk: bool, secure: bool) -> (Secs, Secs) {
    match (kiosk, secure) {
        (true, _) => (KIOSK_IDLE, KIOSK_ABSOLUTE),
        (false, false) => (PERSONAL_PLAIN_IDLE, PERSONAL_PLAIN_ABSOLUTE),
        (false, true) => (PERSONAL_SECURE_IDLE, PERSONAL_SECURE_ABSOLUTE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_never_longer_than_plain() {
        for kiosk in [true, false] {
            let (pi, pa) = session_limits(kiosk, false);
            let (si, sa) = session_limits(kiosk, true);
            assert!(si <= pi && sa <= pa);
        }
    }
}
