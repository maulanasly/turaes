//! Resource sampling for native processes.
//!
//! Two sources, in preference order:
//! 1. **cgroup v2** — precise per-unit accounting for systemd-managed apps
//!    (`/sys/fs/cgroup/system.slice/{app}.service/`).
//! 2. **`/proc`** — per-process fallback for the embedded supervisor.

/// A resource reading for one app.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ResourceStats {
    /// Resident memory in bytes.
    pub mem_bytes: u64,
    /// Cumulative CPU time in microseconds.
    pub cpu_usage_usec: u64,
}

/// CPU percent between two cumulative readings.
///
/// Not normalised by core count (so 250% means "2.5 cores busy"), matching the
/// convention used by docker stats and systemd-cgtop.
pub fn cpu_percent(prev_usec: u64, curr_usec: u64, elapsed_secs: f64) -> f64 {
    if elapsed_secs <= 0.0 || curr_usec < prev_usec {
        return 0.0;
    }
    let delta = (curr_usec - prev_usec) as f64;
    (delta / (elapsed_secs * 1_000_000.0)) * 100.0
}

/// Parse the first whitespace-separated number of a cgroup scalar file.
pub fn parse_scalar(body: &str) -> Option<u64> {
    body.split_whitespace().next()?.parse().ok()
}

/// Parse `usage_usec` from a cgroup v2 `cpu.stat` body.
pub fn parse_cpu_stat(body: &str) -> Option<u64> {
    for line in body.lines() {
        let mut it = line.split_whitespace();
        if it.next() == Some("usage_usec") {
            return it.next()?.parse().ok();
        }
    }
    None
}

/// Read a systemd unit's cgroup v2 stats. Linux only; `None` elsewhere.
pub async fn read_cgroup(unit_dir: &std::path::Path) -> Option<ResourceStats> {
    let mem = tokio::fs::read_to_string(unit_dir.join("memory.current"))
        .await
        .ok()
        .and_then(|b| parse_scalar(&b))
        .unwrap_or(0);
    let cpu = tokio::fs::read_to_string(unit_dir.join("cpu.stat"))
        .await
        .ok()
        .and_then(|b| parse_cpu_stat(&b))
        .unwrap_or(0);
    if mem == 0 && cpu == 0 {
        return None;
    }
    Some(ResourceStats {
        mem_bytes: mem,
        cpu_usage_usec: cpu,
    })
}

/// Minimal `/proc/<pid>/stat` fields turaes cares about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcStat {
    /// User-mode CPU ticks.
    pub utime: u64,
    /// Kernel-mode CPU ticks.
    pub stime: u64,
    /// Resident set size in pages.
    pub rss_pages: i64,
}

/// Parse `/proc/<pid>/stat`, tolerating spaces inside the `(comm)` field.
pub fn parse_proc_stat(body: &str) -> Option<ProcStat> {
    let close = body.rfind(')')?;
    let rest = body.get(close + 1..)?.trim();
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After comm/state, fields are 1-indexed in proc(5): utime=14, stime=15,
    // rss=24. `fields[0]` is field 3 (state), so subtract 3.
    let get = |n: usize| fields.get(n - 3).copied();
    Some(ProcStat {
        utime: get(14)?.parse().ok()?,
        stime: get(15)?.parse().ok()?,
        rss_pages: get(24)?.parse().ok()?,
    })
}

/// Convert ticks + page count to a [`ResourceStats`] reading.
pub fn proc_to_stats(stat: ProcStat, ticks_per_sec: u64, page_size: u64) -> ResourceStats {
    let ticks = stat.utime + stat.stime;
    ResourceStats {
        mem_bytes: (stat.rss_pages.max(0) as u64) * page_size,
        cpu_usage_usec: ticks.saturating_mul(1_000_000) / ticks_per_sec.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_math() {
        // 0.5 core for 1s
        assert!((cpu_percent(0, 500_000, 1.0) - 50.0).abs() < 1e-9);
        assert_eq!(cpu_percent(100, 50, 1.0), 0.0);
        assert_eq!(cpu_percent(0, 100, 0.0), 0.0);
    }

    #[test]
    fn parses_cpu_stat() {
        let body = "usage_usec 12345\nuser_usec 10000\nsystem_usec 2345\n";
        assert_eq!(parse_cpu_stat(body), Some(12345));
        assert_eq!(parse_cpu_stat("no usage here"), None);
    }

    #[test]
    fn parses_proc_stat_with_spaces_in_comm() {
        // Real /proc/<pid>/stat: pid (comm) state ...
        let body = "1234 (my app) S 1 1234 1234 0 -1 4194304 100 0 0 0 7 3 0 0 20 0 5 0 100 1000000 250 18446744073709551615";
        let stat = parse_proc_stat(body).unwrap();
        assert_eq!(stat.utime, 7);
        assert_eq!(stat.stime, 3);
        assert_eq!(stat.rss_pages, 250);
    }

    #[test]
    fn ticks_and_pages_to_stats() {
        let stat = ProcStat {
            utime: 50,
            stime: 50,
            rss_pages: 100,
        };
        let out = proc_to_stats(stat, 100, 4096);
        assert_eq!(out.cpu_usage_usec, 1_000_000);
        assert_eq!(out.mem_bytes, 100 * 4096);
    }
}
