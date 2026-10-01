//! `turaes-runtime` — run applications as native processes.
//!
//! Nothing in this crate knows about containers: an app is a prebuilt binary
//! (M0) supervised either by generated systemd units or by the embedded
//! supervisor, with the choice abstracted behind [`runtime::Runtime`].

pub mod deploy;
pub mod proc;
pub mod runtime;
pub mod systemd;

pub use deploy::{artifact_hash, DeployOutcome, Deployer};
pub use runtime::{AppSpec, RunState, Runtime};
