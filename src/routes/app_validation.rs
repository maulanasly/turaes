//! Shared application-draft validation for create, update, preflight and apply.
//!
//! Every check reuses the same primitives as the authoritative handlers
//! (`validate_name`, `validate_limits`, `ensure_port_free`, quota guards,
//! `validate_domain`, `ensure_domain_available`, plus the `resolve_kind_shape`
//! rules mirrored rule-for-rule) so preflight can never disagree with submit.
//! Unlike the handlers — which fail fast on the first problem — validation
//! here runs every check independently and collects *all* problems, plus
//! advisory warnings and a per-check pass/fail/unknown summary.
//!
//! The handlers stay authoritative: create/update run this validation first
//! (returning every problem at once) and then execute their original flow,
//! which revalidates and remains the final word.

use std::collections::BTreeMap;

use serde::Serialize;
use turaes_core::db::Pool;
use turaes_core::{Error, FieldIssue, Result};

use crate::routes::{apps, domains, quotas};
use crate::state::AppState;

/// Effective values for one draft, after PATCH tri-state resolution (update)
/// or direct mapping (create). Empty domains are normalized to `None`.
pub struct CheckedDraft {
    /// Slug; updates never change it (no `name` field on PATCH).
    pub name: String,
    /// Effective kind. Updates keep the existing kind (immutable).
    pub kind: String,
    /// Effective executable source: `argv[0]` when a command is set.
    pub binary_path: Option<String>,
    /// Effective flat args (`None` when a command supersedes them).
    pub args: Option<String>,
    /// Effective argv command.
    pub command: Option<Vec<String>>,
    /// Effective static source directory.
    pub publish_dir: Option<String>,
    /// Effective loopback port (`0` means none).
    pub port: i64,
    /// Effective primary domain (`None` means no route).
    pub domain: Option<String>,
    /// Effective placement.
    pub server_id: String,
    /// Effective runtime.
    pub runtime: String,
    /// Effective resource ceilings.
    pub mem_limit_mb: Option<i64>,
    /// Effective CPU ceiling.
    pub cpu_quota_pct: Option<i64>,
    /// Effective metrics path.
    pub metrics_path: Option<String>,
    /// App id to exclude from clash checks (`""` on create).
    pub except_app_id: String,
    /// True on create. Gates rules the update flow never enforces: name
    /// uniqueness, binary/port presence, and args+command exclusion (an
    /// update supersedes args instead of rejecting them).
    pub is_create: bool,
    /// Whether a changed domain should consume budget (create with a domain,
    /// or update to a different domain).
    pub check_domain_budget: bool,
    /// Owning organization, for quota checks.
    pub org_id: String,
}

/// Advisory (non-blocking) finding, e.g. "no public route configured".
#[derive(Debug, Clone, Serialize)]
pub struct DraftWarning {
    /// Request field the warning relates to, when it maps to one.
    pub field: Option<String>,
    /// Human-readable detail.
    pub detail: String,
}

/// Preflight result: always HTTP 200; `ok` is false when `errors` is non-empty.
#[derive(Debug, Serialize)]
pub struct PreflightReport {
    /// True when the draft would pass every blocking check.
    pub ok: bool,
    /// Blocking problems, each naming its field.
    pub errors: Vec<FieldIssue>,
    /// Advisory findings that do not block submit.
    pub warnings: Vec<DraftWarning>,
    /// Per-check outcome: `pass`, `fail`, or `unknown` (cannot be verified
    /// from the control plane, e.g. paths on a remote node).
    pub checks: BTreeMap<String, String>,
}

/// Convert any handler error into a field-scoped issue, keeping the
/// handler's own field when it already has one.
pub fn error_to_issue(error: Error, fallback_field: &str) -> FieldIssue {
    match error {
        Error::FieldValidation { field, detail } => FieldIssue {
            field,
            code: "bad_request".into(),
            detail,
        },
        Error::FieldConflict { field, detail } => FieldIssue {
            field,
            code: "conflict".into(),
            detail,
        },
        Error::Conflict(detail) => FieldIssue {
            field: fallback_field.into(),
            code: "conflict".into(),
            detail,
        },
        other => FieldIssue {
            field: fallback_field.into(),
            code: other.code().into(),
            detail: other.to_string(),
        },
    }
}

fn warn(field: Option<&str>, detail: impl Into<String>) -> DraftWarning {
    DraftWarning {
        field: field.map(str::to_string),
        detail: detail.into(),
    }
}

fn issue(field: &str, code: &str, detail: impl Into<String>) -> FieldIssue {
    FieldIssue {
        field: field.into(),
        code: code.into(),
        detail: detail.into(),
    }
}

/// A primary domain or alias already claimed elsewhere. Comparisons are
/// case-insensitive; `except_app_id` excludes the app being edited.
pub(crate) async fn ensure_domain_available(
    pool: &Pool,
    domain: &str,
    except_app_id: &str,
) -> Result<()> {
    let lower = domain.to_ascii_lowercase();
    let primary: Option<String> = sqlx::query_scalar(
        "SELECT name FROM applications WHERE LOWER(domain) = ? AND id != ? LIMIT 1",
    )
    .bind(&lower)
    .bind(except_app_id)
    .fetch_optional(pool)
    .await?;
    if let Some(name) = primary {
        return Err(Error::FieldConflict {
            field: "domain".into(),
            detail: format!("domain '{domain}' is already used by '{name}'"),
        });
    }
    let alias: Option<String> = sqlx::query_scalar(
        "SELECT application_id FROM domains WHERE LOWER(domain) = ? AND application_id != ? LIMIT 1",
    )
    .bind(&lower)
    .bind(except_app_id)
    .fetch_optional(pool)
    .await?;
    if alias.is_some() {
        return Err(Error::FieldConflict {
            field: "domain".into(),
            detail: format!("domain '{domain}' is already used as an alias"),
        });
    }
    Ok(())
}

/// Run every draft check independently and collect all problems.
/// Returns `(errors, warnings, checks)`.
pub async fn validate_draft(
    state: &AppState,
    draft: &CheckedDraft,
) -> (Vec<FieldIssue>, Vec<DraftWarning>, BTreeMap<String, String>) {
    let mut errors: Vec<FieldIssue> = Vec::new();
    let mut warnings: Vec<DraftWarning> = Vec::new();
    let mut failed: Vec<&'static str> = Vec::new();
    let check = |failed: &mut Vec<&'static str>,
                 errors: &mut Vec<FieldIssue>,
                 name: &'static str,
                 issue: FieldIssue| {
        if !failed.contains(&name) {
            failed.push(name);
        }
        errors.push(issue);
    };

    // Name format, then uniqueness (create only; updates cannot rename).
    match apps::validate_name(&draft.name) {
        Ok(()) => {
            if draft.is_create {
                let clash: Option<String> = sqlx::query_scalar(
                    "SELECT id FROM applications WHERE name = ? AND id != ? LIMIT 1",
                )
                .bind(&draft.name)
                .bind(&draft.except_app_id)
                .fetch_optional(&state.pool)
                .await
                .unwrap_or(None);
                if clash.is_some() {
                    check(
                        &mut failed,
                        &mut errors,
                        "name",
                        error_to_issue(
                            Error::Conflict("an application with that name already exists".into()),
                            "name",
                        ),
                    );
                }
            }
        }
        Err(e) => check(&mut failed, &mut errors, "name", error_to_issue(e, "name")),
    }

    // Workload shape. Mirrors `resolve_kind_shape` rule-for-rule (including the
    // on-disk executable check for argv commands); each rule appends
    // independently so one bad field hides no other.
    if !matches!(draft.kind.as_str(), "service" | "static" | "worker") {
        check(
            &mut failed,
            &mut errors,
            "kind",
            issue("kind", "bad_request", "choose service, static or worker"),
        );
    }
    if draft.is_create && draft.args.is_some() && draft.command.is_some() {
        check(
            &mut failed,
            &mut errors,
            "kind",
            issue(
                "args",
                "bad_request",
                "cannot be combined with command; command already contains its arguments",
            ),
        );
    }
    if draft
        .binary_path
        .as_deref()
        .is_some_and(|b| !b.trim().is_empty())
        && draft.command.is_some()
    {
        check(
            &mut failed,
            &mut errors,
            "kind",
            issue(
                "binary_path",
                "bad_request",
                "cannot be combined with command; command[0] is the executable",
            ),
        );
    }
    if let Some(argv) = &draft.command {
        if argv.is_empty() || argv.iter().any(|a| a.trim().is_empty()) {
            check(
                &mut failed,
                &mut errors,
                "kind",
                issue(
                    "command",
                    "bad_request",
                    "must contain a non-empty executable and arguments",
                ),
            );
        } else if argv[0].contains(' ') {
            check(
                &mut failed,
                &mut errors,
                "kind",
                issue(
                    "command",
                    "bad_request",
                    "the executable path cannot contain spaces; no shell is used",
                ),
            );
        } else if tokio::fs::metadata(&argv[0]).await.is_err() {
            check(
                &mut failed,
                &mut errors,
                "kind",
                issue(
                    "command",
                    "bad_request",
                    format!("executable '{}' does not exist or is not readable", argv[0]),
                ),
            );
        }
    }
    if draft.command.is_none() {
        if draft.kind == "static" {
            if draft
                .publish_dir
                .as_deref()
                .is_none_or(|d| d.trim().is_empty())
            {
                check(
                    &mut failed,
                    &mut errors,
                    "kind",
                    issue(
                        "publish_dir",
                        "bad_request",
                        "is required for a static site",
                    ),
                );
            }
        } else if draft.is_create
            && draft
                .binary_path
                .as_deref()
                .is_none_or(|b| b.trim().is_empty())
        {
            // Updates never enforce binary presence (the path is immutable
            // there); only creates do.
            check(
                &mut failed,
                &mut errors,
                "kind",
                issue(
                    "binary_path",
                    "bad_request",
                    "is required for this workload",
                ),
            );
        }
    }
    if draft.kind == "static"
        && (draft
            .binary_path
            .as_deref()
            .is_some_and(|b| !b.trim().is_empty())
            || draft.command.is_some())
    {
        check(
            &mut failed,
            &mut errors,
            "kind",
            issue(
                "kind",
                "bad_request",
                "static sites use a source directory, not a binary or command",
            ),
        );
    }
    if draft.kind != "static"
        && draft
            .publish_dir
            .as_deref()
            .is_some_and(|d| !d.trim().is_empty())
    {
        check(
            &mut failed,
            &mut errors,
            "kind",
            issue(
                "publish_dir",
                "bad_request",
                "is only valid for a static site",
            ),
        );
    }
    if draft.kind == "worker" {
        if draft.port != 0 {
            check(
                &mut failed,
                &mut errors,
                "kind",
                issue(
                    "port",
                    "bad_request",
                    "workers do not listen on a port; leave it empty",
                ),
            );
        }
        if draft.domain.as_deref().is_some_and(|d| !d.is_empty()) {
            check(
                &mut failed,
                &mut errors,
                "kind",
                issue(
                    "domain",
                    "bad_request",
                    "workers have no public route; leave this empty",
                ),
            );
        }
        if draft.metrics_path.is_some() {
            check(
                &mut failed,
                &mut errors,
                "kind",
                issue(
                    "metrics_path",
                    "bad_request",
                    "workers do not expose an HTTP metrics route",
                ),
            );
        }
    } else if draft.is_create && draft.port == 0 {
        // Updates keep the existing port (which may predate this rule).
        check(
            &mut failed,
            &mut errors,
            "kind",
            issue(
                "port",
                "bad_request",
                "is required for a service or static site",
            ),
        );
    }

    // Runtime value.
    if !matches!(draft.runtime.as_str(), "systemd" | "proc") {
        check(
            &mut failed,
            &mut errors,
            "runtime",
            issue("runtime", "bad_request", "choose systemd or turaes (proc)"),
        );
    }

    // Placement target exists.
    let server_exists: i64 = sqlx::query_scalar("SELECT count(*) FROM servers WHERE id = ?")
        .bind(&draft.server_id)
        .fetch_optional(&state.pool)
        .await
        .unwrap_or(Some(0))
        .unwrap_or(0);
    if server_exists == 0 {
        check(
            &mut failed,
            &mut errors,
            "server",
            issue(
                "server_id",
                "bad_request",
                format!("server '{}' does not exist", draft.server_id),
            ),
        );
    }

    // Loopback port (and blue/green pair) availability.
    if draft.port != 0 {
        match apps::ensure_port_free(
            &state.pool,
            draft.port,
            state.cfg.runtime.slot_offset as i64,
            &draft.except_app_id,
        )
        .await
        {
            Ok(()) => {}
            Err(e) => check(&mut failed, &mut errors, "port", error_to_issue(e, "port")),
        }
    }

    // Resource limits, then quota budgets.
    match apps::validate_limits(draft.mem_limit_mb, draft.cpu_quota_pct, &draft.runtime) {
        Ok(()) => {}
        Err(e) => check(
            &mut failed,
            &mut errors,
            "limits",
            error_to_issue(e, "runtime"),
        ),
    }
    match quotas::ensure_capacity(
        &state.pool,
        &draft.org_id,
        &draft.except_app_id,
        draft.mem_limit_mb,
        draft.cpu_quota_pct,
    )
    .await
    {
        Ok(()) => {}
        Err(e) => {
            let issue = match &e {
                Error::Conflict(detail) if detail.starts_with("memory quota") => {
                    error_to_issue(e, "mem_limit_mb")
                }
                Error::Conflict(detail) if detail.starts_with("CPU quota") => {
                    error_to_issue(e, "cpu_quota_pct")
                }
                _ => error_to_issue(e, "quota"),
            };
            check(&mut failed, &mut errors, "quota", issue);
        }
    }

    // Primary domain: format, budget, and global availability. Budget is
    // only consumed by a new or changed domain, mirroring the handlers.
    match draft.domain.as_deref().filter(|d| !d.trim().is_empty()) {
        Some(domain) => match domains::validate_domain(domain) {
            Ok(normalized) => {
                if draft.check_domain_budget {
                    if let Err(e) = quotas::ensure_domain_capacity(&state.pool, &draft.org_id).await
                    {
                        check(
                            &mut failed,
                            &mut errors,
                            "domain",
                            error_to_issue(e, "domain"),
                        );
                    }
                }
                if let Err(e) =
                    ensure_domain_available(&state.pool, &normalized, &draft.except_app_id).await
                {
                    check(
                        &mut failed,
                        &mut errors,
                        "domain",
                        error_to_issue(e, "domain"),
                    );
                }
            }
            Err(e) => check(
                &mut failed,
                &mut errors,
                "domain",
                error_to_issue(e, "domain"),
            ),
        },
        None => {
            if draft.kind != "worker" {
                warnings.push(warn(
                    Some("domain"),
                    "No public domain is configured; the app will have no public route.",
                ));
            }
        }
    }

    // Launch source presence. Advisory only: create accepts a missing binary
    // (deploy fails later), so this never blocks — except argv executables,
    // which the shape check above already rejects.
    let source_path: Option<String> = draft
        .command
        .as_ref()
        .and_then(|argv| argv.first().cloned())
        .or_else(|| {
            if draft.kind == "static" {
                draft.publish_dir.clone()
            } else {
                draft.binary_path.clone()
            }
        });
    let source = match (draft.server_id.as_str(), source_path) {
        (server, _) if server != "local" => {
            warnings.push(warn(
                Some("server_id"),
                "Source paths on a remote node cannot be verified from the control plane.",
            ));
            "unknown"
        }
        (_, Some(path)) if path.trim().is_empty() => "unknown",
        (_, Some(path)) => match tokio::fs::metadata(&path).await {
            Ok(_) => "pass",
            Err(_) => {
                warnings.push(warn(
                    None,
                    format!(
                        "Source '{path}' was not found on this host; it must exist before deploy."
                    ),
                ));
                "unknown"
            }
        },
        (_, None) => "unknown",
    };

    let mut checks = BTreeMap::new();
    for name in [
        "name", "kind", "runtime", "server", "port", "limits", "quota", "domain",
    ] {
        checks.insert(
            name.to_string(),
            if failed.contains(&name) {
                "fail".to_string()
            } else {
                "pass".to_string()
            },
        );
    }
    checks.insert("source".to_string(), source.to_string());
    (errors, warnings, checks)
}
