//! The systemd sandbox (`deploy/turaes.service`, `ProtectSystem=strict`) makes
//! the whole OS tree read-only except `ReadWritePaths`. Every directory the
//! deploy path writes with default config (`config/default.toml [runtime]`)
//! must be covered, or the first turaes-managed deploy fails with EROFS
//! ("io error: Read-only file system (os error 30)").
//!
//! Cargo runs integration tests with CWD set to the package root, so the
//! repo-relative paths below resolve both locally and in CI.

use std::path::Path;

fn read(name: &str) -> String {
    std::fs::read_to_string(name).unwrap_or_else(|e| panic!("cannot read {name}: {e}"))
}

/// Split all `ReadWritePaths=` entries into individual absolute paths.
fn sandbox_writes(unit: &str) -> Vec<String> {
    unit.lines()
        .filter_map(|l| l.trim().strip_prefix("ReadWritePaths="))
        .flat_map(|v| v.split_whitespace().map(str::to_string))
        .collect()
}

/// Extract the `[runtime]` string values (`unit_dir`, `bin_dir`, `state_dir`,
/// `env_dir`, `artifact_dir`) from the default TOML.
fn default_runtime_dirs(toml: &str) -> Vec<(String, String)> {
    let mut in_runtime = false;
    let mut out = Vec::new();
    for line in toml.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_runtime = t == "[runtime]";
            continue;
        }
        if !in_runtime || t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut kv = t.splitn(2, '=');
        let (k, v) = (kv.next().unwrap().trim(), kv.next().unwrap_or("").trim());
        if matches!(
            k,
            "unit_dir" | "bin_dir" | "state_dir" | "env_dir" | "artifact_dir"
        ) {
            let val = v.trim_matches('"').to_string();
            assert!(!val.is_empty(), "empty default for runtime.{k}");
            out.push((k.to_string(), val));
        }
    }
    assert!(!out.is_empty(), "no [runtime] dirs found in default.toml");
    out
}

/// True when `entry` (a sandbox path) covers `path` (equal or ancestor).
fn covers(entry: &str, path: &str) -> bool {
    path == entry || path.starts_with(&format!("{entry}/"))
}

#[test]
fn sandbox_covers_default_runtime_dirs() {
    let writes = sandbox_writes(&read("deploy/turaes.service"));
    assert!(
        !writes.is_empty(),
        "no ReadWritePaths in deploy/turaes.service"
    );
    // ProtectSystem=strict must stay on: the fix is coverage, not removal.
    assert!(
        read("deploy/turaes.service").contains("ProtectSystem=strict"),
        "expected ProtectSystem=strict to stay enabled"
    );
    let dirs = default_runtime_dirs(&read("config/default.toml"));
    // Per-app state is a dynamic subdir ({state_dir}/{app}); probe one level
    // down so ancestor coverage is actually exercised.
    let mut missing = Vec::new();
    for (key, dir) in &dirs {
        let probe = if key == "state_dir" {
            format!("{dir}/probe-app")
        } else {
            dir.clone()
        };
        if !writes.iter().any(|e| covers(e, &probe)) {
            missing.push(format!("runtime.{key}={dir}"));
        }
        // Sanity: entries must be absolute paths.
        for e in &writes {
            assert!(
                Path::new(e).is_absolute(),
                "relative ReadWritePaths entry: {e}"
            );
        }
    }
    assert!(
        missing.is_empty(),
        "deploy/turaes.service ReadWritePaths {writes:?} does not cover: {missing:?}"
    );
}
