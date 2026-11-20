//! `wss-core` — the engine of the Wax Service Shell (WSS).
//!
//! WSS is where packs are made: it turns ZIMs, folders of HTML and documents,
//! EPUB books, PDFs and websites into signed, searchable `.wax` packs, and
//! keeps them current by publishing only what changed. This crate is the
//! engine and the `wss` command line; a desktop app or the SaaS is a front
//! end over the same calls (WSS spec: "everything in the UI is also in the
//! command line").
//!
//! The API a front end uses:
//!
//! * [`Recipe`] — every choice that shapes a pack, serialisable as TOML.
//! * [`detect`] / [`convert`] — recognise a source and build a pack.
//! * [`refresh`] — re-convert a source as the next edition of a pack and
//!   append only what changed.
//! * [`manage`] — [`inspect`](manage::inspect), [`diff`](manage::diff),
//!   [`update`](manage::update), [`rollback`](manage::rollback),
//!   [`gc`](manage::gc), [`sign`](manage::sign), [`verify`](manage::verify).
//!
//! Every result is a plain `Serialize` value, so a GUI shows it without
//! parsing text. Importers are prioritised ZIM > HTML folder > EPUB > PDF >
//! website, and everything they could not carry is reported, never dropped
//! quietly ([`report`]).

pub mod build;
pub mod engine;
pub mod importers;
pub mod manage;
pub mod recipe;
pub mod refs;
pub mod report;
pub mod stage;

pub use engine::{convert, detect, refresh, ConvertOutcome, RefreshOutcome, SourceKind};
pub use recipe::Recipe;
