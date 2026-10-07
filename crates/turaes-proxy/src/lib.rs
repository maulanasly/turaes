//! `turaes-proxy` — the in-process reverse proxy.
//!
//! Host-based routing is always available (and unit-tested); the Pingora data
//! plane is behind the `pingora` feature so the rest of the workspace builds
//! fast on any platform. TLS certificates are issued by certbot and loaded from
//! disk — turaes never speaks ACME itself.

pub mod maintenance;
pub mod router;
pub mod service;
pub mod tls;

pub use router::{ParkedHost, ParkedReason, RouteTable, Router, Upstream};
pub use service::{pingora_enabled, state, ProxyState};
pub use tls::{CertPaths, CertStore};
