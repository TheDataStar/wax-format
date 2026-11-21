//! Random values, hashes and codes.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::policy;

/// `n` bytes from the operating system's secure random source.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).expect("the OS random source failed");
    b
}

pub fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

/// A bearer token: 256 random bits as 64 lowercase hex characters. Used for
/// session ids, browser ids, device credentials and tickets (all at least
/// the 128 bits privilege boundary §4.2 asks for).
pub fn token() -> String {
    hex(&random_bytes(32))
}

/// A short public id (a device id shown to a teacher): 64 bits, 16 hex.
pub fn short_id() -> String {
    hex(&random_bytes(8))
}

/// What the database keeps instead of a bearer token: its SHA-256. The
/// tokens are 256 random bits, so a plain hash cannot be reversed; a copy
/// of the database hands over no live session (privilege boundary §4.2).
pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Whether `s` looks like a token this module made (refuse anything else
/// before it reaches the database).
pub fn is_token(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Crockford's base-32 alphabet: no I, L, O or U, so a code read aloud or
/// typed from paper is not misread.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A new sign-in card code, `policy::CARD_CODE_LEN` characters.
pub fn card_code() -> String {
    // rejection-free: each byte's low 5 bits are uniform over 32 symbols
    random_bytes(policy::CARD_CODE_LEN).iter().map(|b| CROCKFORD[(b & 31) as usize] as char).collect()
}

/// A typed code made canonical: upper case, spaces and dashes dropped, and
/// the letters Crockford folds (O→0, I and L→1). `None` if anything else
/// is left or the length is wrong.
pub fn normalize_card_code(s: &str) -> Option<String> {
    let mut out = String::new();
    for c in s.chars() {
        let c = c.to_ascii_uppercase();
        let c = match c {
            ' ' | '-' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !CROCKFORD.contains(&(c as u8)) || !c.is_ascii() {
            return None;
        }
        out.push(c);
    }
    (out.len() == policy::CARD_CODE_LEN).then_some(out)
}

/// `XXXXX-XXXXX`, for printing.
pub fn display_card_code(code: &str) -> String {
    let (a, b) = code.split_at(code.len() / 2);
    format!("{a}-{b}")
}

/// A card code is short (50 bits), so a plain hash in a stolen database
/// could be brute-forced. It is kept as an HMAC under a key held in its own
/// file, which a copy of the database alone does not contain.
pub fn card_mac(key: &[u8], code: &str) -> String {
    let mut m = <Hmac<Sha256>>::new_from_slice(key).expect("hmac takes any key length");
    m.update(code.as_bytes());
    hex(&m.finalize().into_bytes())
}

/// A password for a passworded person: Argon2id with the policy's cost.
pub fn hash_password(password: &str) -> String {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let salt = SaltString::encode_b64(&random_bytes(16)).expect("16 bytes is a valid salt");
    argon2().hash_password(password.as_bytes(), &salt).expect("argon2 with valid parameters").to_string()
}

/// Check a password against a stored hash. Takes the same time whether or
/// not the person exists when called with [`DUMMY_HASH`].
pub fn verify_password(password: &str, stored: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    match PasswordHash::new(stored) {
        Ok(h) => argon2().verify_password(password.as_bytes(), &h).is_ok(),
        Err(_) => false,
    }
}

/// Verified against when no such person exists, so a wrong name costs the
/// same time as a wrong password.
pub fn dummy_hash() -> &'static str {
    use std::sync::OnceLock;
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| hash_password("not a real password, only spends the time"))
}

fn argon2() -> argon2::Argon2<'static> {
    let p = argon2::Params::new(policy::ARGON2_M_KIB, policy::ARGON2_T, policy::ARGON2_P, None)
        .expect("policy's argon2 parameters are valid");
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, p)
}

/// A generated first-admin password: 20 Crockford characters (100 bits) in
/// four groups, readable off a screen and typeable.
pub fn generated_password() -> String {
    let c: String = random_bytes(20).iter().map(|b| CROCKFORD[(b & 31) as usize] as char).collect();
    format!("{}-{}-{}-{}", &c[0..5], &c[5..10], &c[10..15], &c[15..20])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_fold() {
        let c = card_code();
        assert_eq!(c.len(), 10);
        assert_eq!(normalize_card_code(&display_card_code(&c)).unwrap(), c);
        assert_eq!(normalize_card_code("abcde-fghjk").unwrap(), "ABCDEFGHJK");
        assert_eq!(normalize_card_code("o0il1 2345 6").unwrap(), "0011123456");
        assert!(normalize_card_code("ABCDE-FGHJ").is_none());
        assert!(normalize_card_code("ABCDE-FGHJU").is_none());
    }

    #[test]
    fn passwords() {
        let h = hash_password("correct horse battery");
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &h));
        assert!(!verify_password("wrong", &h));
        assert!(!verify_password("x", "not a hash"));
    }

    #[test]
    fn tokens() {
        let t = token();
        assert!(is_token(&t));
        assert_ne!(token_hash(&t), t);
        assert!(!is_token("abc"));
    }
}
