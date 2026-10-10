//! Versioned artifact registry: CI pushes binaries/bundles, deploys pull by
//! version.
//!
//! A push carries **bytes + provenance only**. The app record stays the sole
//! authority for runtime config (port, health, domain, env): runtime-flavored
//! push fields are ignored and reported back as `ignored_hints`, never
//! applied. Deploys resolve `version`/`channel` to an immutable content hash
//! and join the existing deploy path there.
//!
//! Two package shapes (no new system deps; gzip + tar are pure Rust):
//! a single ELF binary as raw bytes, or a gzip tarball with a top-level
//! `manifest.json` (allowlisted keys only).
//! Upload auth is the existing Developer floor (a session or a `turaes_*`
//! `deploy`-scoped token); the OIDC trusted-publisher exchange arrives
//! separately.

use std::collections::HashMap;
use std::io::Read;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use futures::StreamExt;
use serde::Deserialize;

use turaes_core::db::Pool;
use turaes_core::models::{Application, Release};
use turaes_core::{Error, Result};

use crate::audit;
use crate::authz::{self, CurrentUser, Role};
use crate::state::AppState;

/// Upload ceiling (M4 backlog): the repo-wide axum default is 2 MiB, so the
/// route disables it and enforces this while streaming.
pub const MAX_UPLOAD_BYTES: u64 = 256 * 1024 * 1024;
/// `manifest.json` ceiling inside bundles.
const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// Query-string fields the upload endpoint understands. Anything else that
/// looks like runtime config is collected into `ignored_hints`, never applied;
/// anything else entirely is reported as `ignored_unknown` so misspelled
/// fields (e.g. `versoin=`) fail loudly instead of silently.
const KNOWN_UPLOAD_FIELDS: &[&str] = &[
    "app",
    "repo",
    "version",
    "commit",
    "arch",
    "build_url",
    "notes",
    "filename",
];
/// Runtime-flavored keys a push may carry; always ignored, always reported.
const RUNTIME_HINT_FIELDS: &[&str] = &[
    "port",
    "health_path",
    "metrics_path",
    "domain",
    "env",
    "command",
    "args",
    "runtime",
    "server_id",
    "workdir",
];

/// `manifest.json` keys a bundle may declare. Anything else is rejected.
const MANIFEST_FIELDS: &[&str] = &["name", "version", "arch", "commit", "files"];

/// Body for `POST .../apps/{id}/deploy` (also used by the deploy handler in
/// `routes::apps`): deploy the pinned hash for a release instead of
/// re-hashing the app's configured binary path.
#[derive(Debug, Default, Deserialize)]
pub struct DeployBody {
    /// Exact release version (`1.2.3`).
    pub version: Option<String>,
    /// Channel pointer (`stable`, `latest`, …).
    pub channel: Option<String>,
}

/// Resolve `version` xor `channel` to its release plus artifact hash.
/// Yanked releases never resolve.
pub(crate) async fn resolve_release(
    pool: &Pool,
    org_id: &str,
    app_id: &str,
    version: Option<&str>,
    channel: Option<&str>,
) -> Result<(Release, String)> {
    match (version, channel) {
        (Some(_), Some(_)) => Err(Error::FieldValidation {
            field: "channel".into(),
            detail: "pass exactly one of version or channel".into(),
        }),
        (None, None) => Err(Error::BadRequest("pass version or channel".into())),
        (Some(v), None) => {
            let release: Option<Release> = sqlx::query_as(
                "SELECT * FROM releases WHERE org_id = ? AND application_id = ? AND version = ?",
            )
            .bind(org_id)
            .bind(app_id)
            .bind(v)
            .fetch_optional(pool)
            .await?;
            let release =
                release.ok_or_else(|| Error::NotFound(format!("release {v} not found")))?;
            if release.is_yanked {
                return Err(Error::NotFound(format!("release {v} was yanked")));
            }
            let hash = artifact_hash(pool, &release.artifact_id).await?;
            Ok((release, hash))
        }
        (None, Some(c)) => {
            let release: Option<Release> = sqlx::query_as(
                "SELECT r.* FROM release_channels ch \
                 JOIN releases r ON r.id = ch.release_id \
                 WHERE ch.org_id = ? AND ch.application_id = ? AND ch.channel = ? \
                 AND r.is_yanked = 0",
            )
            .bind(org_id)
            .bind(app_id)
            .bind(c)
            .fetch_optional(pool)
            .await?;
            let release =
                release.ok_or_else(|| Error::NotFound(format!("no release on channel {c}")))?;
            let hash = artifact_hash(pool, &release.artifact_id).await?;
            Ok((release, hash))
        }
    }
}

async fn artifact_hash(pool: &Pool, artifact_id: &str) -> Result<String> {
    sqlx::query_scalar("SELECT hash FROM artifacts WHERE id = ?")
        .bind(artifact_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| Error::Internal("release points at a missing artifact".into()))
}

/// Fetch an app of this org by id or name (push callers know names; the
/// dashboard knows ids).
pub(crate) async fn fetch_app_ref(pool: &Pool, org_id: &str, app_ref: &str) -> Result<Application> {
    sqlx::query_as::<_, Application>(
        "SELECT * FROM applications WHERE org_id = ? AND (id = ? OR name = ?)",
    )
    .bind(org_id)
    .bind(app_ref)
    .bind(app_ref)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| Error::NotFound(format!("application {app_ref}")))
}

/// Strict `MAJOR.MINOR.PATCH` with optional `-pre` / `+build`. Channel names
/// are never valid versions.
pub(crate) fn validate_version(v: &str) -> Result<()> {
    if v.is_empty() || v.len() > 64 {
        return Err(version_err(v));
    }
    let (core_pre, build) = match v.split_once('+') {
        Some((c, b)) => (c, Some(b)),
        None => (v, None),
    };
    if let Some(b) = build {
        if b.is_empty()
            || b.split('.').any(|id| {
                id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            })
        {
            return Err(version_err(v));
        }
    }
    if core_pre.is_empty() || core_pre.ends_with('.') || core_pre.ends_with('-') {
        return Err(version_err(v));
    }
    let (core, pre) = match core_pre.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (core_pre, None),
    };
    let nums: Vec<&str> = core.split('.').collect();
    if nums.len() != 3
        || nums
            .iter()
            .any(|n| n.is_empty() || n.len() > 10 || !n.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(version_err(v));
    }
    if let Some(pre) = pre {
        if pre.is_empty()
            || pre.split('.').any(|id| {
                id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(version_err(v));
        }
    }
    Ok(())
}

fn version_err(v: &str) -> Error {
    Error::FieldValidation {
        field: "version".into(),
        detail: format!("version '{v}' is not strict MAJOR.MINOR.PATCH semver"),
    }
}

/// Channel names: short lowercase slugs (`stable`, `latest`, `beta`, …).
pub(crate) fn validate_channel(c: &str) -> Result<()> {
    let ok = !c.is_empty()
        && c.len() <= 32
        && c.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(Error::FieldValidation {
            field: "channel".into(),
            detail: format!("channel '{c}' must be 1-32 chars of [a-z0-9-_]"),
        })
    }
}

/// CI repo identity: `owner/name`.
pub(crate) fn validate_repo(repo: &str) -> Result<()> {
    let mut parts = repo.split('/');
    let ok = repo.len() <= 200
        && matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None) if {
            let good = |s: &str| {
                !s.is_empty()
                    && s.len() <= 100
                    && s.bytes().all(|c| {
                        c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.'
                    })
            };
            good(a) && good(b)
        });
    if ok {
        Ok(())
    } else {
        Err(Error::FieldValidation {
            field: "repo".into(),
            detail: format!("repo '{repo}' must look like owner/name"),
        })
    }
}

/// Inspect ELF headers: require an ELF binary and report its architecture.
/// Returns `Ok(None)` for valid ELF on an unrecognized machine.
fn elf_arch(bytes: &[u8]) -> Result<Option<&'static str>> {
    if bytes.len() < 20 || &bytes[0..4] != b"\x7fELF" {
        return Err(Error::BadRequest(
            "upload is not an ELF binary (service binaries only in v1)".into(),
        ));
    }
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    match machine {
        62 => Ok(Some("x86_64")),
        183 => Ok(Some("aarch64")),
        _ => Ok(None),
    }
}

/// Ensure an `artifacts` metadata row exists for `hash` (idempotent adopt),
/// returning its id.
async fn ensure_artifact_row(
    pool: &Pool,
    org_id: &str,
    hash: &str,
    size_bytes: i64,
    arch: Option<&str>,
    actor: &str,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT OR IGNORE INTO artifacts (id, org_id, hash, size_bytes, media_type, arch, created_by) \
         VALUES (?, ?, ?, ?, 'application/octet-stream', ?, ?)",
    )
    .bind(&id)
    .bind(org_id)
    .bind(hash)
    .bind(size_bytes)
    .bind(arch)
    .bind(actor)
    .execute(pool)
    .await?;
    sqlx::query_scalar("SELECT id FROM artifacts WHERE hash = ?")
        .bind(hash)
        .fetch_one(pool)
        .await
        .map_err(Error::Db)
}

/// Move a channel pointer to a release (upsert).
async fn set_channel(
    pool: &Pool,
    org_id: &str,
    app_id: &str,
    channel: &str,
    release_id: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO release_channels (org_id, application_id, channel, release_id, updated_at) \
         VALUES (?, ?, ?, ?, datetime('now')) \
         ON CONFLICT(org_id, application_id, channel) DO UPDATE SET \
         release_id = excluded.release_id, updated_at = datetime('now')",
    )
    .bind(org_id)
    .bind(app_id)
    .bind(channel)
    .bind(release_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// `POST /api/v1/orgs/{org}/registry/links` — link a CI repo to an app
/// (Developer). Pushes claiming any other repo are rejected.
pub async fn create_link(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
    Json(input): Json<HashMap<String, String>>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
    let app_ref = input.get("application_id").cloned().unwrap_or_default();
    let repo = input.get("repo").cloned().unwrap_or_default();
    if app_ref.is_empty() {
        return Err(Error::FieldValidation {
            field: "application_id".into(),
            detail: "application_id is required".into(),
        });
    }
    validate_repo(&repo)?;
    let app = fetch_app_ref(&state.pool, &org_id, &app_ref).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM registry_links WHERE org_id = ? AND application_id = ? AND repo = ?",
    )
    .bind(&org_id)
    .bind(&app.id)
    .bind(&repo)
    .fetch_one(&state.pool)
    .await?;
    if exists > 0 {
        return Err(Error::FieldConflict {
            field: "repo".into(),
            detail: format!("repo '{repo}' is already linked to '{}'", app.name),
        });
    }
    sqlx::query(
        "INSERT INTO registry_links (id, org_id, application_id, repo, created_by) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&org_id)
    .bind(&app.id)
    .bind(&repo)
    .bind(&user.login)
    .execute(&state.pool)
    .await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&app.id),
        "registry.link",
        Some("registry_link"),
        Some(&id),
        Some(&serde_json::json!({"repo": repo}).to_string()),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "link": {"id": id, "application_id": app.id, "repo": repo},
        })),
    ))
}

/// `DELETE /api/v1/orgs/{org}/registry/links?application_id=&repo=` —
/// unlink a CI repo (Developer).
pub async fn delete_link(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<StatusCode> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
    let app_ref = params.get("application_id").cloned().unwrap_or_default();
    let repo = params.get("repo").cloned().unwrap_or_default();
    let app = fetch_app_ref(&state.pool, &org_id, &app_ref).await?;
    let deleted = sqlx::query(
        "DELETE FROM registry_links WHERE org_id = ? AND application_id = ? AND repo = ?",
    )
    .bind(&org_id)
    .bind(&app.id)
    .bind(&repo)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(Error::NotFound(format!(
            "no link from repo '{repo}' to '{}'",
            app.name
        )));
    }
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&app.id),
        "registry.unlink",
        Some("registry_link"),
        None,
        Some(&serde_json::json!({"repo": repo}).to_string()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/orgs/{org}/registry/{app}/links` — linked CI repos (Viewer).
pub async fn list_links(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, app_ref)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let app = fetch_app_ref(&state.pool, &org_id, &app_ref).await?;
    let links: Vec<turaes_core::models::RegistryLink> = sqlx::query_as(
        "SELECT * FROM registry_links WHERE org_id = ? AND application_id = ? ORDER BY repo ASC",
    )
    .bind(&org_id)
    .bind(&app.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "links": links })))
}

/// Stream the request body to a temp file (256 MiB cap), returning its path
/// and size. Single pass; hashing happens at store time via `put_file`.
async fn buffer_upload(state: &AppState, body: Body) -> Result<(std::path::PathBuf, u64)> {
    let dir = state.artifacts.root().join("sha256");
    tokio::fs::create_dir_all(&dir).await?;
    let tmp = dir.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    let mut out = tokio::fs::File::create(&tmp).await?;
    let mut total: u64 = 0;
    let mut stream = body.into_data_stream();
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| Error::BadRequest(format!("failed to read upload body: {e}")))?;
        total += chunk.len() as u64;
        if total > MAX_UPLOAD_BYTES {
            drop(out);
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(Error::BadRequest(format!(
                "upload exceeds the {} MiB limit",
                MAX_UPLOAD_BYTES / 1024 / 1024
            )));
        }
        out.write_all(&chunk).await?;
    }
    out.flush().await?;
    drop(out);
    if total == 0 {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(Error::BadRequest("upload body is empty".into()));
    }
    // Service binaries must stay executable end to end: `put_file` preserves
    // the source mode bits, so the staging copy carries 0755 from here.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(&tmp, perms).await?;
    }
    Ok((tmp, total))
}

/// Read the first bytes of a staged upload (magic sniffing without loading
/// a potentially 256 MiB file into memory).
async fn read_head(tmp: &std::path::Path) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(tmp).await?;
    let mut head = vec![0u8; 64];
    let n = file.read(&mut head).await?;
    head.truncate(n);
    Ok(head)
}

/// Unpacked bundle: manifest fields plus `(archive path, bytes)` members.
type UnpackedBundle = (HashMap<String, serde_json::Value>, Vec<(String, Vec<u8>)>);

/// Unpack a gzip tarball in a blocking task: returns `(manifest, members)`
/// where members are `(archive path, bytes)` for every `files/` entry.
fn unpack_bundle(tmp: std::path::PathBuf) -> Result<UnpackedBundle> {
    let data = std::fs::read(&tmp).map_err(turaes_core::Error::Io)?;
    if data.len() < 2 || data[0] != 0x1f || data[1] != 0x8b {
        return Err(Error::BadRequest("bundle is not a gzip tarball".into()));
    }
    let decoder = flate2::read::GzDecoder::new(&data[..]);
    let mut archive = tar::Archive::new(decoder);
    let mut manifest: Option<Vec<u8>> = None;
    let mut members: Vec<(String, Vec<u8>)> = Vec::new();
    let mut entries = archive
        .entries()
        .map_err(|e| Error::BadRequest(format!("cannot read bundle entries: {e}")))?;
    for item in entries.by_ref() {
        let mut entry = item.map_err(|e| Error::BadRequest(format!("bad bundle entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| Error::BadRequest(format!("bad bundle path: {e}")))?
            .to_string_lossy()
            .into_owned();
        if path == "manifest.json" {
            if manifest.is_some() {
                return Err(Error::BadRequest(
                    "bundle has two manifest.json files".into(),
                ));
            }
            let mut buf = Vec::new();
            entry
                .take(MAX_MANIFEST_BYTES as u64 + 1)
                .read_to_end(&mut buf)
                .map_err(|e| Error::BadRequest(format!("cannot read manifest.json: {e}")))?;
            if buf.len() > MAX_MANIFEST_BYTES {
                return Err(Error::BadRequest("manifest.json exceeds 64 KiB".into()));
            }
            manifest = Some(buf);
            continue;
        }
        if !path.starts_with("files/") || path.ends_with('/') {
            continue;
        }
        let rel = path["files/".len()..].to_string();
        if rel.is_empty()
            || rel.starts_with('/')
            || rel.split('/').any(|c| c == ".." || c.is_empty())
        {
            return Err(Error::BadRequest(format!(
                "bundle path '{path}' escapes files/"
            )));
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| Error::BadRequest(format!("cannot read bundle file '{path}': {e}")))?;
        members.push((rel, buf));
    }
    let manifest = manifest
        .ok_or_else(|| Error::BadRequest("bundle has no top-level manifest.json".into()))?;
    let parsed: HashMap<String, serde_json::Value> = serde_json::from_slice(&manifest)
        .map_err(|e| Error::BadRequest(format!("manifest.json is not valid JSON: {e}")))?;
    Ok((parsed, members))
}

/// `POST /api/v1/orgs/{org}/registry/artifacts` — push bytes (Developer).
///
/// Raw body bytes; metadata rides the query string:
/// `?app=&repo=&version=&commit=&arch=&build_url=&notes=&filename=`.
/// Single ELF binaries and gzip tarballs (with `manifest.json`) are accepted.
/// Responds `201` with the stored hash, the created release (when `version`
/// is given), and any `ignored_hints` — runtime-flavored fields that were
/// **not** applied.
pub async fn upload(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    body: Body,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
    let app_ref = params.get("app").cloned().unwrap_or_default();
    if app_ref.is_empty() {
        return Err(Error::FieldValidation {
            field: "app".into(),
            detail: "app id or name is required".into(),
        });
    }
    let app = fetch_app_ref(&state.pool, &org_id, &app_ref).await?;
    let repo = params.get("repo").cloned().unwrap_or_default();
    if repo.is_empty() {
        return Err(Error::FieldValidation {
            field: "repo".into(),
            detail: "repo (owner/name) is required so the push can be authorized".into(),
        });
    }
    validate_repo(&repo)?;
    let linked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry_links WHERE org_id = ? AND application_id = ? AND repo = ?",
    )
    .bind(&org_id)
    .bind(&app.id)
    .bind(&repo)
    .fetch_one(&state.pool)
    .await?;
    if linked == 0 {
        return Err(Error::Forbidden(format!(
            "repo '{repo}' is not linked to '{}' — link it in app Settings first",
            app.name
        )));
    }
    let version = params.get("version").cloned().filter(|v| !v.is_empty());
    if let Some(v) = &version {
        validate_version(v)?;
        let dup: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM releases WHERE org_id = ? AND application_id = ? AND version = ?",
        )
        .bind(&org_id)
        .bind(&app.id)
        .bind(v)
        .fetch_one(&state.pool)
        .await?;
        if dup > 0 {
            return Err(Error::FieldConflict {
                field: "version".into(),
                detail: format!("release {v} already exists for '{}'", app.name),
            });
        }
    }
    let ignored_hints: Vec<String> = RUNTIME_HINT_FIELDS
        .iter()
        .filter(|f| params.contains_key(**f))
        .map(|f| f.to_string())
        .collect();
    let ignored_unknown: Vec<String> = params
        .keys()
        .filter(|k| {
            !KNOWN_UPLOAD_FIELDS.contains(&k.as_str()) && !RUNTIME_HINT_FIELDS.contains(&k.as_str())
        })
        .cloned()
        .collect();

    let (tmp, _size) = buffer_upload(&state, body).await?;
    let head = read_head(&tmp).await?;
    let is_bundle = head.len() >= 2 && head[0] == 0x1f && head[1] == 0x8b;

    // Effective release metadata: explicit query fields win, the manifest
    // fills gaps, and direct conflicts are rejected instead of silently
    // resolved. (The query `version` was already semver- and dup-checked
    // before buffering the body.)
    let mut release_version = version.clone();
    let mut release_arch = params.get("arch").cloned();
    let mut release_commit = params.get("commit").cloned().filter(|s| !s.is_empty());

    // (primary hash, files_json entries, arch)
    let (primary_hash, files_json, arch): (String, Vec<serde_json::Value>, Option<String>) =
        if is_bundle {
            let (manifest, members) = tokio::task::spawn_blocking(move || unpack_bundle(tmp))
                .await
                .map_err(|e| Error::Internal(format!("bundle unpack failed: {e}")))??;
            for key in manifest.keys() {
                if !MANIFEST_FIELDS.contains(&key.as_str()) {
                    return Err(Error::FieldValidation {
                        field: "manifest.json".into(),
                        detail: format!("unknown manifest key '{key}'"),
                    });
                }
            }
            // Manifest/app agreement: identity fields must match the push, or the
            // push is ambiguous about what it is.
            let str_field = |k: &str| manifest.get(k).and_then(|v| v.as_str()).map(str::to_string);
            if let Some(name) = str_field("name") {
                if name != app.name {
                    return Err(Error::FieldValidation {
                        field: "manifest.json".into(),
                        detail: format!("manifest name '{name}' does not match app '{}'", app.name),
                    });
                }
            }
            if let Some(mv) = str_field("version") {
                match &release_version {
                    Some(v) if v != &mv => {
                        return Err(Error::FieldValidation {
                            field: "version".into(),
                            detail: format!(
                                "query version '{v}' conflicts with manifest version '{mv}'"
                            ),
                        })
                    }
                    Some(_) => {}
                    None => {
                        validate_version(&mv)?;
                        release_version = Some(mv);
                    }
                }
            }
            if let Some(ma) = str_field("arch") {
                match &release_arch {
                    Some(a) if a != &ma => {
                        return Err(Error::FieldValidation {
                            field: "arch".into(),
                            detail: format!("query arch '{a}' conflicts with manifest arch '{ma}'"),
                        })
                    }
                    Some(_) => {}
                    None => {
                        release_arch = Some(ma);
                    }
                }
            }
            if release_commit.is_none() {
                release_commit = str_field("commit");
            }
            // A manifest-sourced version skipped the pre-upload dup check.
            if version.is_none() {
                if let Some(v) = &release_version {
                    let dup: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM releases WHERE org_id = ? AND application_id = ? AND version = ?",
                )
                .bind(&org_id)
                .bind(&app.id)
                .bind(v)
                .fetch_one(&state.pool)
                .await?;
                    if dup > 0 {
                        return Err(Error::FieldConflict {
                            field: "version".into(),
                            detail: format!("release {v} already exists for '{}'", app.name),
                        });
                    }
                }
            }
            let declared: Vec<serde_json::Value> = manifest
                .get("files")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if declared.is_empty() && members.is_empty() {
                return Err(Error::FieldValidation {
                    field: "manifest.json".into(),
                    detail: "bundle declares no files and carries none under files/".into(),
                });
            }
            // Store every member; the first is the deployable file.
            let mut hashes: Vec<String> = Vec::new();
            let mut entries: Vec<serde_json::Value> = Vec::new();
            let ordered: Vec<(String, Vec<u8>)> = if declared.is_empty() {
                let mut m = members;
                m.sort_by(|a, b| a.0.cmp(&b.0));
                m
            } else {
                let mut by_path: HashMap<String, Vec<u8>> = members.into_iter().collect();
                let mut out = Vec::new();
                for d in &declared {
                    let p = d.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                        Error::FieldValidation {
                            field: "manifest.json".into(),
                            detail: "every files[] entry needs a path".into(),
                        }
                    })?;
                    let bytes = by_path.remove(p).ok_or_else(|| Error::FieldValidation {
                        field: "manifest.json".into(),
                        detail: format!("declared file '{p}' is missing from files/"),
                    })?;
                    out.push((p.to_string(), bytes));
                }
                out
            };
            if ordered.is_empty() {
                return Err(Error::FieldValidation {
                    field: "manifest.json".into(),
                    detail: "bundle carries no installable files".into(),
                });
            }
            let mut arch_out: Option<String> = None;
            for (i, (rel, bytes)) in ordered.iter().enumerate() {
                if i == 0 {
                    arch_out = elf_arch(bytes)?.map(str::to_string);
                }
                let member_tmp = state
                    .artifacts
                    .root()
                    .join("sha256")
                    .join(format!(".tmp-{}-{i}", uuid::Uuid::new_v4()));
                tokio::fs::write(&member_tmp, bytes).await?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(0o755);
                    tokio::fs::set_permissions(&member_tmp, perms).await?;
                }
                let hash = state.artifacts.put_file(&member_tmp).await?;
                let _ = tokio::fs::remove_file(&member_tmp).await;
                entries.push(serde_json::json!({"path": rel, "hash": hash}));
                hashes.push(hash);
            }
            if let Some(want) = &release_arch {
                if let Some(got) = &arch_out {
                    if got != want {
                        return Err(Error::FieldValidation {
                            field: "arch".into(),
                            detail: format!(
                                "declared arch '{want}' does not match ELF machine ({got})"
                            ),
                        });
                    }
                }
            }
            let primary_hash = hashes.remove(0);
            (primary_hash, entries, arch_out)
        } else {
            let arch_found = elf_arch(&head)?.map(str::to_string);
            if let Some(want) = &release_arch {
                if let Some(got) = &arch_found {
                    if got != want {
                        return Err(Error::FieldValidation {
                            field: "arch".into(),
                            detail: format!(
                                "declared arch '{want}' does not match ELF machine ({got})"
                            ),
                        });
                    }
                }
            }
            let hash = state.artifacts.put_file(&tmp).await?;
            let _ = tokio::fs::remove_file(&tmp).await;
            (hash, vec![], arch_found)
        };

    // Prefer the declared arch (already cross-checked against ELF); fall
    // back to what the binary headers reported.
    let arch = release_arch.or(arch);

    let size_bytes: i64 = state
        .artifacts
        .path_for(&primary_hash)
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    let artifact_id = ensure_artifact_row(
        &state.pool,
        &org_id,
        &primary_hash,
        size_bytes,
        arch.as_deref(),
        &user.login,
    )
    .await?;

    // Adopt bundle members into the metadata table so GC pins them too.
    for entry in &files_json {
        if let (Some(h),) = (entry.get("hash").and_then(|v| v.as_str()),) {
            if h != primary_hash {
                let sz: i64 = state
                    .artifacts
                    .path_for(h)
                    .ok()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len() as i64)
                    .unwrap_or(0);
                ensure_artifact_row(&state.pool, &org_id, h, sz, arch.as_deref(), &user.login)
                    .await?;
            }
        }
    }

    let commit = release_commit;
    let build_url = params.get("build_url").cloned().filter(|s| !s.is_empty());
    let notes = params.get("notes").cloned().filter(|s| !s.is_empty());
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&app.id),
        "artifact.push",
        Some("artifact"),
        Some(&artifact_id),
        Some(&serde_json::json!({"hash": primary_hash, "repo": repo}).to_string()),
    )
    .await?;

    let release = match release_version {
        Some(v) => {
            let id = uuid::Uuid::new_v4().to_string();
            let files_str = serde_json::to_string(&files_json).unwrap_or_else(|_| "[]".into());
            sqlx::query(
                "INSERT INTO releases (id, org_id, application_id, version, artifact_id, \
                 commit_sha, build_url, notes, files_json, created_by) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(&org_id)
            .bind(&app.id)
            .bind(&v)
            .bind(&artifact_id)
            .bind(&commit)
            .bind(&build_url)
            .bind(&notes)
            .bind(&files_str)
            .bind(&user.login)
            .execute(&state.pool)
            .await?;
            // No auto-promotion: `latest` moves only via the explicit,
            // Admin-gated promote call, so a push can never silently
            // redirect the channel other deploys resolve.
            audit::record(
                &state,
                Some(&org_id),
                Some(&user),
                Some(&app.id),
                "release.push",
                Some("release"),
                Some(&id),
                Some(&serde_json::json!({"version": v, "hash": primary_hash}).to_string()),
            )
            .await?;
            Some(serde_json::json!({
                "id": id, "version": v, "hash": primary_hash,
                "files": files_json,
            }))
        }
        None => None,
    };

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "artifact": {"hash": primary_hash, "size_bytes": size_bytes, "arch": arch},
            "release": release,
            "ignored_hints": ignored_hints,
            "ignored_unknown": ignored_unknown,
        })),
    ))
}

/// `POST /api/v1/orgs/{org}/registry/releases` — pin an existing stored
/// hash to a version (Developer).
pub async fn create_release(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(org): Path<String>,
    Json(input): Json<HashMap<String, String>>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
    let app_ref = input.get("application_id").cloned().unwrap_or_default();
    let version = input.get("version").cloned().unwrap_or_default();
    let hash = input.get("artifact_hash").cloned().unwrap_or_default();
    let app = fetch_app_ref(&state.pool, &org_id, &app_ref).await?;
    validate_version(&version)?;
    if hash.is_empty() || !state.artifacts.has(&hash) {
        return Err(Error::FieldValidation {
            field: "artifact_hash".into(),
            detail: "artifact_hash must name a blob already in the store".into(),
        });
    }
    let dup: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM releases WHERE org_id = ? AND application_id = ? AND version = ?",
    )
    .bind(&org_id)
    .bind(&app.id)
    .bind(&version)
    .fetch_one(&state.pool)
    .await?;
    if dup > 0 {
        return Err(Error::FieldConflict {
            field: "version".into(),
            detail: format!("release {version} already exists for '{}'", app.name),
        });
    }
    let size_bytes: i64 = state
        .artifacts
        .path_for(&hash)
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    let artifact_id =
        ensure_artifact_row(&state.pool, &org_id, &hash, size_bytes, None, &user.login).await?;
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO releases (id, org_id, application_id, version, artifact_id, \
         commit_sha, build_url, notes, created_by) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&org_id)
    .bind(&app.id)
    .bind(&version)
    .bind(&artifact_id)
    .bind(input.get("commit_sha").cloned().filter(|s| !s.is_empty()))
    .bind(input.get("build_url").cloned().filter(|s| !s.is_empty()))
    .bind(input.get("notes").cloned().filter(|s| !s.is_empty()))
    .bind(&user.login)
    .execute(&state.pool)
    .await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&app.id),
        "release.push",
        Some("release"),
        Some(&id),
        Some(&serde_json::json!({"version": version, "hash": hash}).to_string()),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "release": {"id": id, "version": version, "hash": hash},
        })),
    ))
}

async fn fetch_org_release(pool: &Pool, org_id: &str, id: &str) -> Result<Release> {
    sqlx::query_as::<_, Release>(
        "SELECT r.* FROM releases r \
         JOIN applications a ON a.id = r.application_id \
         WHERE r.id = ? AND a.org_id = ?",
    )
    .bind(id)
    .bind(org_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| Error::NotFound(format!("release {id}")))
}

/// `POST /api/v1/orgs/{org}/registry/releases/{id}/promote` — move a
/// channel pointer (Admin). Promoting never redeploys by itself; apps pinned
/// to the channel pick it up on their next deploy.
pub async fn promote(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
    Json(input): Json<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
    let channel = input.get("channel").cloned().unwrap_or_default();
    validate_channel(&channel)?;
    let release = fetch_org_release(&state.pool, &org_id, &id).await?;
    if release.is_yanked {
        return Err(Error::BadRequest(format!(
            "release {} is yanked and cannot be promoted",
            release.version
        )));
    }
    set_channel(
        &state.pool,
        &org_id,
        &release.application_id,
        &channel,
        &release.id,
    )
    .await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&release.application_id),
        "release.promote",
        Some("release"),
        Some(&release.id),
        Some(&serde_json::json!({"version": release.version, "channel": channel}).to_string()),
    )
    .await?;
    Ok(Json(serde_json::json!({
        "channel": channel, "version": release.version, "release_id": release.id,
    })))
}

/// `POST /api/v1/orgs/{org}/registry/releases/{id}/yank` — hide a release
/// from every channel (Admin). Bytes are kept for running deploys.
pub async fn yank(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Admin).await?;
    let release = fetch_org_release(&state.pool, &org_id, &id).await?;
    sqlx::query("UPDATE releases SET is_yanked = 1 WHERE id = ?")
        .bind(&release.id)
        .execute(&state.pool)
        .await?;
    sqlx::query("DELETE FROM release_channels WHERE release_id = ?")
        .bind(&release.id)
        .execute(&state.pool)
        .await?;
    audit::record(
        &state,
        Some(&org_id),
        Some(&user),
        Some(&release.application_id),
        "release.yank",
        Some("release"),
        Some(&release.id),
        Some(&serde_json::json!({"version": release.version}).to_string()),
    )
    .await?;
    Ok(Json(serde_json::json!({
        "yanked": release.version, "release_id": release.id,
    })))
}

/// One row of the releases list: release fields plus the artifact hash.
#[derive(Debug, sqlx::FromRow)]
struct ReleaseListRow {
    id: String,
    version: String,
    commit_sha: Option<String>,
    build_url: Option<String>,
    notes: Option<String>,
    hash: String,
    is_yanked: i64,
    created_by: Option<String>,
    created_at: String,
}

/// `GET /api/v1/orgs/{org}/registry/{app}/releases` — releases + channel
/// pointers (Viewer).
pub async fn list_releases(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, app_ref)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let app = fetch_app_ref(&state.pool, &org_id, &app_ref).await?;
    let rows: Vec<ReleaseListRow> = sqlx::query_as(
        "SELECT r.id, r.version, r.commit_sha, r.build_url, r.notes, a.hash, \
         r.is_yanked, r.created_by, r.created_at \
         FROM releases r JOIN artifacts a ON a.id = r.artifact_id \
         WHERE r.org_id = ? AND r.application_id = ? \
         ORDER BY r.created_at DESC, r.version DESC LIMIT 200",
    )
    .bind(&org_id)
    .bind(&app.id)
    .fetch_all(&state.pool)
    .await?;
    let releases: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id, "version": r.version, "hash": r.hash,
                "commit_sha": r.commit_sha, "build_url": r.build_url, "notes": r.notes,
                "is_yanked": r.is_yanked != 0, "created_by": r.created_by,
                "created_at": r.created_at,
            })
        })
        .collect();
    let channels: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT ch.channel, r.version, ch.release_id FROM release_channels ch \
         JOIN releases r ON r.id = ch.release_id \
         WHERE ch.org_id = ? AND ch.application_id = ? ORDER BY ch.channel ASC",
    )
    .bind(&org_id)
    .bind(&app.id)
    .fetch_all(&state.pool)
    .await?;
    let channels: Vec<serde_json::Value> = channels
        .into_iter()
        .map(|(channel, version, release_id)| {
            serde_json::json!({"channel": channel, "version": version, "release_id": release_id})
        })
        .collect();
    Ok(Json(
        serde_json::json!({ "releases": releases, "channels": channels }),
    ))
}

/// `GET /api/v1/orgs/{org}/registry/{app}/resolve?version=&channel=` —
/// preview what a version/channel pins to, without deploying (Viewer).
pub async fn resolve(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path((org, app_ref)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>> {
    let org_id = authz::authorize_org(&state, &user, &org, Role::Viewer).await?;
    let app = fetch_app_ref(&state.pool, &org_id, &app_ref).await?;
    let (release, hash) = resolve_release(
        &state.pool,
        &org_id,
        &app.id,
        params.get("version").map(String::as_str),
        params.get("channel").map(String::as_str),
    )
    .await?;
    Ok(Json(serde_json::json!({
        "version": release.version, "hash": hash, "release_id": release.id,
        "commit_sha": release.commit_sha, "channel": params.get("channel"),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_accept_strict_semver() {
        for v in [
            "1.0.0",
            "0.1.0",
            "10.20.30",
            "1.0.0-rc.1",
            "2.0.0-beta.2+x.y",
            "1.2.3+build.5",
        ] {
            validate_version(v).unwrap_or_else(|_| panic!("{v} should be valid"));
        }
    }

    #[test]
    fn versions_reject_channels_and_slop() {
        for v in [
            "",
            "latest",
            "stable",
            "v1.2.3",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "1..3",
            "1.2.3-",
            "1.2.3+",
            "1.2.3-rc..1",
            "1.2.3-rc_1",
        ] {
            assert!(validate_version(v).is_err(), "{v} should be rejected");
        }
    }

    #[test]
    fn channels_and_repos_have_shapes() {
        for c in ["stable", "latest", "beta-2", "env_prod"] {
            validate_channel(c).unwrap();
        }
        for c in ["", "Stable", "a b", "x".repeat(33).as_str()] {
            assert!(validate_channel(c).is_err());
        }
        validate_repo("acme/regapp").unwrap();
        validate_repo("a-b_c.d/e-f_g.h").unwrap();
        for r in ["", "noslash", "a/b/c", "/b", "a/", "a b/c"] {
            assert!(validate_repo(r).is_err(), "{r} should be rejected");
        }
    }

    #[test]
    fn elf_headers_report_arch() {
        let mut good = vec![0u8; 64];
        good[0..4].copy_from_slice(b"\x7fELF");
        good[18..20].copy_from_slice(&62u16.to_le_bytes());
        assert_eq!(elf_arch(&good).unwrap(), Some("x86_64"));
        good[18..20].copy_from_slice(&183u16.to_le_bytes());
        assert_eq!(elf_arch(&good).unwrap(), Some("aarch64"));
        assert!(elf_arch(b"#!/bin/sh\n").is_err());
        assert!(elf_arch(&[]).is_err());
    }
}
