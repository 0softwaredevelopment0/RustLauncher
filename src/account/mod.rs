//! Accounts and authentication.
//!
//! - [`auth`] — signing in (offline, Ely.by, Microsoft) and the [`Account`]
//!   model;
//! - [`store`] — the persisted account list (`AccountStore`);
//! - [`db`] — the portable SQLite database with Argon2id password hashes.

pub mod auth;
pub mod db;
pub mod store;

pub use auth::{
    login_offline, offline_uuid, validate_username, Account, AccountKind,
};
pub use store::AccountStore;
