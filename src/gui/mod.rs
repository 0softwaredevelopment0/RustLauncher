//! The eframe/egui GUI, split into focused modules:
//!
//! - `state` — the `App` struct, shared types, screens and the version model;
//! - `freestanding` — version merge/sort/filter, process plumbing, helpers;
//! - `screens` — General / Console / Instances / Versions / Servers /
//!   Accounts / Skins;
//! - `content_ui` — the Modrinth browser;
//! - `toast_ui` — the notification stack and painter-drawn icons;
//! - `app` — the eframe entry point and per-frame orchestration;
//! - `tests` — GUI unit tests.

pub(crate) mod app;
pub use state::App;
mod content_ui;
mod freestanding;
mod screens;
pub(crate) mod state;
#[cfg(test)]
mod tests;
pub(crate) mod toast_ui;
