//! Host → upstream routing table.
//!
//! The table is immutable behind an `ArcSwap`, so the request hot path never
//! locks while deploys rebuild and publish a new snapshot.

use std::collections::HashMap;

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};

/// Where a request should be forwarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upstream {
    /// Loopback host, usually `127.0.0.1`.
    pub host: String,
    /// Application port.
    pub port: u16,
    /// Terminate TLS for this upstream (drives SNI cert lookup).
    pub tls: bool,
}

impl Upstream {
    /// `host:port` for peer construction.
    pub fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// An immutable snapshot of the routing table.
#[derive(Debug, Clone, Default)]
pub struct RouteTable {
    routes: HashMap<String, Upstream>,
    base_domain: String,
}

impl RouteTable {
    /// Build a table from explicit hostnames and a wildcard base domain.
    pub fn new(base_domain: impl Into<String>, routes: HashMap<String, Upstream>) -> Self {
        Self {
            routes: routes
                .into_iter()
                .map(|(k, v)| (k.to_lowercase(), v))
                .collect(),
            base_domain: base_domain.into().to_lowercase(),
        }
    }

    /// Resolve a `Host` header value (port optional) to an upstream.
    ///
    /// Exact hostname matches win; otherwise `{app}.{base_domain}` resolves the
    /// `{app}` route, enabling wildcard/multitenant subdomains.
    pub fn resolve(&self, host: &str) -> Option<&Upstream> {
        let host = host.split(':').next().unwrap_or(host).trim().to_lowercase();
        if let Some(u) = self.routes.get(&host) {
            return Some(u);
        }
        if !self.base_domain.is_empty() {
            let suffix = format!(".{}", self.base_domain);
            if let Some(label) = host.strip_suffix(&suffix) {
                if let Some(u) = self.routes.get(label) {
                    return Some(u);
                }
            }
        }
        None
    }

    /// Number of configured routes.
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

/// Thread-safe, hot-swappable routing table.
#[derive(Debug)]
pub struct Router {
    table: ArcSwap<RouteTable>,
}

impl Router {
    /// Create a router with a base domain and initial routes.
    pub fn new(base_domain: impl Into<String>, routes: HashMap<String, Upstream>) -> Self {
        Self {
            table: ArcSwap::from_pointee(RouteTable::new(base_domain, routes)),
        }
    }

    /// Publish a new snapshot atomically.
    pub fn publish(&self, table: RouteTable) {
        self.table.store(std::sync::Arc::new(table));
    }

    /// Resolve a host using the current snapshot.
    pub fn resolve(&self, host: &str) -> Option<Upstream> {
        self.table.load().resolve(host).cloned()
    }

    /// Current snapshot length.
    pub fn len(&self) -> usize {
        self.table.load().len()
    }

    /// Whether the current snapshot has no routes.
    pub fn is_empty(&self) -> bool {
        self.table.load().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(port: u16) -> Upstream {
        Upstream {
            host: "127.0.0.1".into(),
            port,
            tls: true,
        }
    }

    fn router() -> Router {
        let mut routes = HashMap::new();
        routes.insert("kalkulator.rayakala.ink".into(), up(8000));
        routes.insert("beruang".into(), up(8000));
        router_with(routes)
    }

    fn router_with(routes: HashMap<String, Upstream>) -> Router {
        Router::new("rayakala.ink", routes)
    }

    #[test]
    fn exact_host_match() {
        let r = router();
        assert_eq!(r.resolve("kalkulator.rayakala.ink").unwrap().port, 8000);
    }

    #[test]
    fn host_with_port_and_case() {
        let r = router();
        assert_eq!(r.resolve("Kalkulator.Rayakala.ink:443").unwrap().port, 8000);
    }

    #[test]
    fn wildcard_subdomain() {
        let r = router();
        assert_eq!(r.resolve("beruang.rayakala.ink").unwrap().port, 8000);
    }

    #[test]
    fn unknown_host_is_none() {
        let r = router();
        assert!(r.resolve("nope.example.com").is_none());
    }

    #[test]
    fn publish_swaps_snapshot() {
        let r = router();
        assert!(r.resolve("new.rayakala.ink").is_none());
        let mut routes = HashMap::new();
        routes.insert("new".into(), up(9001));
        r.publish(RouteTable::new("rayakala.ink", routes));
        assert_eq!(r.resolve("new.rayakala.ink").unwrap().port, 9001);
    }
}
