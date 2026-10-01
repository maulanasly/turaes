//! Downsampling helpers for storing samples into fixed buckets.

use chrono::{DateTime, Utc};

/// Floor a unix-seconds timestamp to the start of its `size_secs` bucket.
pub fn bucket_start(unix_secs: i64, size_secs: i64) -> i64 {
    if size_secs <= 0 {
        return unix_secs;
    }
    unix_secs - unix_secs.rem_euclid(size_secs)
}

/// Floor a timestamp to its minute.
pub fn bucket_minute(ts: DateTime<Utc>) -> i64 {
    bucket_start(ts.timestamp(), 60)
}

/// Running aggregate for one bucket of CPU/memory samples.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Aggregate {
    /// Number of samples seen.
    pub count: u64,
    /// Sum of CPU percentages.
    pub cpu_sum: f64,
    /// Peak memory observed.
    pub mem_max: u64,
}

impl Aggregate {
    /// Add a sample.
    pub fn push(&mut self, cpu_pct: f64, mem_bytes: u64) {
        self.count += 1;
        self.cpu_sum += cpu_pct;
        self.mem_max = self.mem_max.max(mem_bytes);
    }

    /// Mean CPU percent for the bucket.
    pub fn cpu_avg(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.cpu_sum / self.count as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn buckets_floor_correctly() {
        assert_eq!(bucket_start(1_700_000_025, 60), 1_699_999_980);
        assert_eq!(bucket_start(125, 60), 120);
        assert_eq!(bucket_start(120, 60), 120);
        assert_eq!(bucket_start(59, 60), 0);
        assert_eq!(bucket_start(5, 0), 5);
    }

    #[test]
    fn minute_bucket_roundtrips() {
        let ts = Utc.timestamp_opt(1_700_000_045, 0).unwrap();
        assert_eq!(bucket_minute(ts), 1_700_000_040);
    }

    #[test]
    fn aggregate_math() {
        let mut agg = Aggregate::default();
        agg.push(10.0, 100);
        agg.push(30.0, 300);
        assert_eq!(agg.count, 2);
        assert_eq!(agg.cpu_avg(), 20.0);
        assert_eq!(agg.mem_max, 300);
    }
}
