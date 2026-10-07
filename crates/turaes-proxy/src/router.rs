//! Host → upstreams routing table.
//!
//! A host may map to several upstreams (replicas); the data plane picks one.
//! The table is immutable behind an `ArcSwap`, so the request hot path never
//! locks while deploys rebuild and publish a new snapshot.

use std::collections::HashMap;

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};

/// Where a request should be forwarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upstream {
    /// Backend host (loopback or a node's private address).
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

/// Pick a backend round-robin by a monotonically increasing counter.
pub fn pick(upstreams: &[Upstream], counter: u64) -> Option<&Upstream> {
    if upstreams.is_empty() {
        None
    } else {
        Some(&upstreams[(counter as usize) % upstreams.len()])
    }
}

/// Why a known host has no live upstreams. Rendered on the maintenance page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParkedReason {
    /// Stopped by its owner (`app.stop`).
    Stopped,
    /// Failing health checks or a failed supervisor state.
    Unhealthy,
}

/// A hostname kept in the table without upstreams, so the data plane can
/// answer it with a maintenance page instead of a bare 502.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParkedHost {
    /// Application slug, shown on the page (never the raw hostname).
    pub app: String,
    /// Why it is parked.
    pub reason: ParkedReason,
}

/// An immutable snapshot of the routing table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteTable {
    routes: HashMap<String, Vec<Upstream>>,
    parked: HashMap<String, ParkedHost>,
    base_domain: String,
}

fn lookup<'a, V>(map: &'a HashMap<String, V>, base_domain: &str, host: &str) -> Option<&'a V> {
    let host = host.split(':').next().unwrap_or(host).trim().to_lowercase();
    if let Some(v) = map.get(&host) {
        return Some(v);
    }
    if !base_domain.is_empty() {
        let suffix = format!(".{base_domain}");
        if let Some(label) = host.strip_suffix(&suffix) {
            if let Some(v) = map.get(label) {
                return Some(v);
            }
        }
    }
    None
}

impl RouteTable {
    /// Build a table from explicit hostnames and a wildcard base domain.
    pub fn new(base_domain: impl Into<String>, routes: HashMap<String, Vec<Upstream>>) -> Self {
        Self {
            routes: routes
                .into_iter()
                .map(|(k, v)| (k.to_lowercase(), v))
                .collect(),
            parked: HashMap::new(),
            base_domain: base_domain.into().to_lowercase(),
        }
    }

    /// Attach parked hosts (known hostnames without live upstreams).
    pub fn with_parked(mut self, parked: HashMap<String, ParkedHost>) -> Self {
        self.parked = parked
            .into_iter()
            .map(|(k, v)| (k.to_lowercase(), v))
            .collect();
        self
    }

    /// Resolve a `Host` header value (port optional) to all upstreams.
    ///
    /// Exact hostname matches win; otherwise `{app}.{base_domain}` resolves the
    /// `{app}` route, enabling wildcard/multitenant subdomains.
    pub fn resolve_all(&self, host: &str) -> &[Upstream] {
        lookup(&self.routes, &self.base_domain, host).map_or(&[], |v| v)
    }

    /// Parked entry for a host, if it is known but has no live upstreams.
    /// Same exact-then-`base_domain` resolution as [`Self::resolve_all`].
    pub fn is_parked(&self, host: &str) -> Option<ParkedHost> {
        lookup(&self.parked, &self.base_domain, host).cloned()
    }

    /// First upstream for a host (backward-compatible convenience).
    pub fn resolve(&self, host: &str) -> Option<&Upstream> {
        self.resolve_all(host).first()
    }

    /// Number of configured hosts.
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
    pub fn new(base_domain: impl Into<String>, routes: HashMap<String, Vec<Upstream>>) -> Self {
        Self {
            table: ArcSwap::from_pointee(RouteTable::new(base_domain, routes)),
        }
    }

    /// Publish a new snapshot atomically.
    pub fn publish(&self, table: RouteTable) {
        self.table.store(std::sync::Arc::new(table));
    }

    /// Publish only when the snapshot differs; returns whether it changed.
    /// Lets frequent triggers (health transitions, toggles) call for a
    /// rebuild without churning the hot path.
    pub fn publish_if_changed(&self, table: RouteTable) -> bool {
        if **self.table.load() == table {
            return false;
        }
        self.publish(table);
        true
    }

    /// Parked entry for a host using the current snapshot.
    pub fn is_parked(&self, host: &str) -> Option<ParkedHost> {
        self.table.load().is_parked(host)
    }

    /// All upstreams for a host using the current snapshot.
    pub fn resolve_all(&self, host: &str) -> Vec<Upstream> {
        self.table.load().resolve_all(host).to_vec()
    }

    /// First upstream for a host (convenience).
    pub fn resolve(&self, host: &str) -> Option<Upstream> {
        self.table.load().resolve(host).cloned()
    }

    /// Current number of hosts.
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
        let mut routes: HashMap<String, Vec<Upstream>> = HashMap::new();
        routes.insert("kalkulator.rayakala.ink".into(), vec![up(8000)]);
        routes.insert("beruang".into(), vec![up(8000)]);
        routes.insert("replica.test".into(), vec![up(8001), up(8002), up(8003)]);
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
    fn multi_upstream_and_round_robin() {
        let r = router();
        let ups = r.resolve_all("replica.test");
        assert_eq!(ups.len(), 3);
        assert_eq!(pick(&ups, 0).unwrap().port, 8001);
        assert_eq!(pick(&ups, 1).unwrap().port, 8002);
        assert_eq!(pick(&ups, 3).unwrap().port, 8001);
        assert!(pick(&[], 0).is_none());
    }

    #[test]
    fn unknown_host_is_none() {
        let r = router();
        assert!(r.resolve("nope.example.com").is_none());
        assert!(r.resolve_all("nope.example.com").is_empty());
    }

    #[test]
    fn publish_swaps_snapshot() {
        let r = router();
        assert!(r.resolve("new.rayakala.ink").is_none());
        let mut routes: HashMap<String, Vec<Upstream>> = HashMap::new();
        routes.insert("new".into(), vec![up(9001)]);
        r.publish(RouteTable::new("rayakala.ink", routes));
        assert_eq!(r.resolve("new.rayakala.ink").unwrap().port, 9001);
    }

    fn parked_router() -> Router {
        let r = router();
        let mut parked = HashMap::new();
        parked.insert(
            "old.rayakala.ink".into(),
            ParkedHost {
                app: "old".into(),
                reason: ParkedReason::Stopped,
            },
        );
        parked.insert(
            "sick".into(),
            ParkedHost {
                app: "sick".into(),
                reason: ParkedReason::Unhealthy,
            },
        );
        let table = RouteTable::new("rayakala.ink", HashMap::new()).with_parked(parked);
        r.publish(table);
        r
    }

    #[test]
    fn parked_exact_and_wildcard_match() {
        let r = parked_router();
        // Exact hostname, case-insensitive, port tolerated.
        let p = r.is_parked("OLD.rayakala.ink:443").unwrap();
        assert_eq!(p.app, "old");
        assert_eq!(p.reason, ParkedReason::Stopped);
        // Label fallback through the base domain.
        let p = r.is_parked("sick.rayakala.ink").unwrap();
        assert_eq!(p.app, "sick");
        assert_eq!(p.reason, ParkedReason::Unhealthy);
        // Unknown hosts stay unknown; routed hosts are not parked.
        assert!(r.is_parked("nope.example.com").is_none());
        assert!(r.is_parked("kalkulator.rayakala.ink").is_none());
    }

    #[test]
    fn publish_if_changed_skips_identical_snapshots() {
        let r = router();
        let same = RouteTable::new("rayakala.ink", HashMap::new());
        // Current snapshot differs (has routes), so this publishes.
        assert!(r.publish_if_changed(same));
        // Publishing it again is a no-op.
        let again = RouteTable::new("rayakala.ink", HashMap::new());
        assert!(!r.publish_if_changed(again));
        // A parked entry counts as a change.
        let mut parked = HashMap::new();
        parked.insert(
            "old.rayakala.ink".into(),
            ParkedHost {
                app: "old".into(),
                reason: ParkedReason::Stopped,
            },
        );
        let changed = RouteTable::new("rayakala.ink", HashMap::new()).with_parked(parked);
        assert!(r.publish_if_changed(changed));
    }
}
