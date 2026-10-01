//! `turaes-core` — configuration, storage, models and cryptographic helpers
//! shared by every turaes component.
//!
//! turaes is a lightweight, self-hosted PaaS that runs applications as native
//! processes (systemd or an embedded supervisor) instead of containers.

pub mod config;
pub mod crypto;
pub mod db;
pub mod error;
pub mod models;

pub use config::Config;
pub use error::{Error, Result};
