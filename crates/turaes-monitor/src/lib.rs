//! `turaes-monitor` — health probes, Prometheus scraping and native resource
//! sampling.
//!
//! The "monitoring check tonggeret" path lives here: scrape an app's `/metrics`,
//! fold `visitors_total{region}` + `unique_visitors_estimate{region}` into
//! per-region rows, and pair that with cgroup/`/proc` CPU and memory readings.

pub mod health;
pub mod rollup;
pub mod scrape;
pub mod stats;

pub use health::{HealthConfig, HealthOutcome, Threshold, Transition};
pub use rollup::{bucket_minute, bucket_start, Aggregate};
pub use scrape::{parse as parse_prometheus, visitor_samples, Sample, VisitSample};
pub use stats::{cpu_percent, read_cgroup, ResourceStats};
