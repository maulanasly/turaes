//! HTTP health probing and threshold-based state transitions.

use std::time::{Duration, Instant};

use serde::Serialize;

/// Health check tuning for one app.
#[derive(Debug, Clone, Serialize)]
pub struct HealthConfig {
    /// Path to request, e.g. `/health`.
    pub path: String,
    /// Request timeout.
    pub timeout_secs: u64,
    /// Consecutive successes required to flip to healthy.
    pub healthy_threshold: u32,
    /// Consecutive failures required to flip to unhealthy.
    pub unhealthy_threshold: u32,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            path: "/health".into(),
            timeout_secs: 5,
            healthy_threshold: 2,
            unhealthy_threshold: 3,
        }
    }
}

/// Outcome of a single probe.
#[derive(Debug, Clone, Serialize)]
pub struct HealthOutcome {
    /// `healthy` or `unhealthy`.
    pub status: &'static str,
    /// HTTP status code, when a response arrived.
    pub status_code: Option<u16>,
    /// Round-trip time in milliseconds.
    pub response_time_ms: u64,
    /// Error text on failure.
    pub error_message: Option<String>,
}

impl HealthOutcome {
    /// Whether the probe succeeded (2xx).
    pub fn is_healthy(&self) -> bool {
        self.status == "healthy"
    }
}

/// Probe a health URL. A 2xx is healthy; anything else (or a transport error)
/// is unhealthy.
pub async fn probe(client: &reqwest::Client, url: &str, timeout: Duration) -> HealthOutcome {
    let started = Instant::now();
    match client.get(url).timeout(timeout).send().await {
        Ok(resp) => {
            let elapsed = started.elapsed().as_millis() as u64;
            let code = resp.status();
            if code.is_success() {
                HealthOutcome {
                    status: "healthy",
                    status_code: Some(code.as_u16()),
                    response_time_ms: elapsed,
                    error_message: None,
                }
            } else {
                HealthOutcome {
                    status: "unhealthy",
                    status_code: Some(code.as_u16()),
                    response_time_ms: elapsed,
                    error_message: Some(format!("unexpected status {code}")),
                }
            }
        }
        Err(e) => HealthOutcome {
            status: "unhealthy",
            status_code: None,
            response_time_ms: started.elapsed().as_millis() as u64,
            error_message: Some(e.to_string()),
        },
    }
}

/// Tracks consecutive probes and reports only threshold crossings.
#[derive(Debug, Clone, Default)]
pub struct Threshold {
    consecutive_ok: u32,
    consecutive_fail: u32,
    healthy: bool,
}

/// A state transition worth acting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Became healthy.
    BecameHealthy,
    /// Became unhealthy.
    BecameUnhealthy,
}

impl Threshold {
    /// Create a tracker in the unknown (unhealthy) state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current belief.
    pub fn is_healthy(&self) -> bool {
        self.healthy
    }

    /// Record a probe and return a transition when a threshold is crossed.
    pub fn record(&mut self, outcome: &HealthOutcome, cfg: &HealthConfig) -> Option<Transition> {
        if outcome.is_healthy() {
            self.consecutive_ok += 1;
            self.consecutive_fail = 0;
            if !self.healthy && self.consecutive_ok >= cfg.healthy_threshold {
                self.healthy = true;
                return Some(Transition::BecameHealthy);
            }
        } else {
            self.consecutive_fail += 1;
            self.consecutive_ok = 0;
            if self.healthy && self.consecutive_fail >= cfg.unhealthy_threshold {
                self.healthy = false;
                return Some(Transition::BecameUnhealthy);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> HealthOutcome {
        HealthOutcome {
            status: "healthy",
            status_code: Some(200),
            response_time_ms: 5,
            error_message: None,
        }
    }

    fn fail() -> HealthOutcome {
        HealthOutcome {
            status: "unhealthy",
            status_code: None,
            response_time_ms: 5000,
            error_message: Some("timeout".into()),
        }
    }

    #[test]
    fn flips_healthy_after_threshold() {
        let cfg = HealthConfig {
            healthy_threshold: 2,
            unhealthy_threshold: 3,
            ..Default::default()
        };
        let mut t = Threshold::new();
        assert_eq!(t.record(&ok(), &cfg), None);
        assert_eq!(t.record(&ok(), &cfg), Some(Transition::BecameHealthy));
        assert!(t.is_healthy());
    }

    #[test]
    fn flips_unhealthy_after_threshold() {
        let cfg = HealthConfig {
            healthy_threshold: 1,
            unhealthy_threshold: 2,
            ..Default::default()
        };
        let mut t = Threshold::new();
        assert_eq!(t.record(&ok(), &cfg), Some(Transition::BecameHealthy));
        assert_eq!(t.record(&fail(), &cfg), None);
        assert_eq!(t.record(&fail(), &cfg), Some(Transition::BecameUnhealthy));
        assert!(!t.is_healthy());
    }

    #[test]
    fn interleaved_failures_reset() {
        let cfg = HealthConfig {
            healthy_threshold: 2,
            unhealthy_threshold: 3,
            ..Default::default()
        };
        let mut t = Threshold::new();
        t.record(&fail(), &cfg);
        t.record(&fail(), &cfg);
        t.record(&ok(), &cfg); // resets failure streak
        assert_eq!(t.record(&fail(), &cfg), None); // only 1 after reset
        assert!(!t.is_healthy());
    }
}
