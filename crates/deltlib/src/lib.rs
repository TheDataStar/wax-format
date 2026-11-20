//! `deltlib` — the Delt library: installed packs, cross-pack search, and
//! `deltd`, the one server on the box that serves every pack and answers the
//! Library API (`docs/deltd-api-v1.md`).
//!
//! What is built, against the API doc's build plan (§11):
//!
//! * **Step 1, pack serving — complete** except the per-request
//!   `entry_viewed` progress path (step 3): hostname routing and the §4.4
//!   checks, every row of §4.1 (tombstones answer `410 Gone`), the §4.2
//!   headers, one ETag per encoding, byte ranges, zstd passthrough, and
//!   box-side decoding of shared-dictionary entries.
//! * **Step 2, library and search — Library and Search groups and
//!   `/status`.** Sign-in itself is `identityd`'s ([`identity`]).
//!
//! Modules: [`hostname`] (pack hostnames), [`identity`] (what `deltd` asks
//! `identityd`), [`catalog`] (`pack_index`/`pack_generation`), [`library`]
//! (the packs on disk), [`search`] (query building, fusion, completion),
//! [`packzone`], [`api`], [`server`].

pub mod api;
pub mod catalog;
pub mod hostname;
pub mod identity;
pub mod library;
pub mod packzone;
pub mod search;
pub mod server;

pub use catalog::Catalog;
pub use identity::{Identity, Session, StaticIdentity};
pub use library::Library;
pub use server::{router, AppState};
