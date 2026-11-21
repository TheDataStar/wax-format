//! `deltos-identityd`: the box's register of people, roles and sign-ins
//! (contract §4, §5.1; Track F §3, §24; `docs/privilege-boundary.md` §4, §6,
//! §10). Design and decisions: `docs/identityd.md`.
//!
//! * [`register`] — the SQLite register and every rule about sessions,
//!   cards, remembered devices, partitions and tickets;
//! * [`protocol`] — the local socket, its caller checks, and a client;
//! * [`policy`] — the numbers (session lengths, card code, rate limits);
//! * [`secret`] — random values, hashes and codes.

pub mod policy;
pub mod protocol;
pub mod register;
pub mod secret;

pub use protocol::{CallError, Caller, Callers, Client};
pub use register::{Ctx, Error, Grant, Kind, NewProfile, Register, SessionView, SignedIn};
