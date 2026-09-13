//! `zim2wax` — ZIM archive in, `.wax` pack out (DeltOS Track B, component B1).
//!
//! v0 is the lossy text + image converter Track B §4 scopes for P1. The rules
//! it implements live in exactly two places: Track B §20 (canonical paths,
//! manifest derivations, redirect flattening) and the Cross-Track Contract §11
//! (field formats, licensing outcomes, the build report). Where those are
//! silent the module docs say what was chosen and the B1 summary flags it.
//!
//! * [`paths`] — the `(namespace, url)` → canonical `entries.path` rule.
//! * [`rewrite`] — in-document href/src/srcset and CSS `url()` rewriting.
//! * [`lang`] — ISO 639-3 → BCP-47.
//! * [`png`] — the placeholder icon.
//! * [`convert`] — the pipeline, on top of `wax_builder::build_from_entries`.

pub mod convert;
pub mod lang;
pub mod paths;
pub mod png;
pub mod rewrite;

pub use convert::{convert, probe, ConvertOptions, ConvertReport, Derived, ICON_PATH};
