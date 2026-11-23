//! `deltos-shelld`, the box's key holder (privilege boundary §3): the only
//! program that changes the box, and only through its closed list of
//! actions, each checked again against `identityd` whatever the box API
//! already checked. Design and what is not built yet: `docs/shelld.md`.

pub mod action;
pub mod addons;
pub mod check;
pub mod client;
pub mod drives;
pub mod log;
pub mod packs;
pub mod server;

pub use action::Action;
pub use server::{serve, Config, NetPaths, Shell};
