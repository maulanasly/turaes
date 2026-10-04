//! Perimeter defenses for the dashboard server.
//!
//! - **CSRF binding**: cookie-authenticated mutations must carry a JSON
//!   content type. Browsers can be tricked into cross-site form POSTs, but
//!   those cannot set `Content-Type: application/json` without a CORS
//!   preflight — which turaes never grants (there is no CORS layer).
//! - **Rate limits**: global sliding-window buckets (per-IP keying would be
//!   wrong: behind Pingora every client arrives as loopback, and the proxy
//!   does not set trusted `X-Forwarded-For`). Only `/auth/*` and mutating
//!   `/api/*` calls are counted.
//! - **Request timeout**: caps handler futures (deploys legitimately take up
//!   to ~a minute through health-gating and drain, hence 120s).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::auth::SESSION_COOKIE;

/// Window for the sliding-window rate buckets.
const WINDOW: Duration = Duration::from_secs(60);

/// Cap slow handlers (well above the worst legitimate deploy).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

fn has_session_cookie(req: &Request) -> bool {
    req.headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|cookies| {
            cookies
                .split(';')
                .any(|c| c.trim().starts_with(&format!("{SESSION_COOKIE}=")))
        })
}

fn has_bearer(req: &Request) -> bool {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Bearer ") || v.starts_with("bearer "))
}

fn is_json_content_type(req: &Request) -> bool {
    req.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| {
            ct.split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        })
}

fn is_mutation(req: &Request) -> bool {
    matches!(
        req.method(),
        &Method::POST | &Method::PATCH | &Method::PUT | &Method::DELETE
    )
}

/// Reject cookie-driven mutations that do not look like API calls.
/// Bearer-authed and cookie-less requests pass through untouched.
pub async fn csrf(req: Request, next: Next) -> Response {
    if is_mutation(&req)
        && has_session_cookie(&req)
        && !has_bearer(&req)
        && !is_json_content_type(&req)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "detail": "refusing non-JSON cookie-authenticated mutation (CSRF protection)"
            })),
        )
            .into_response();
    }
    next.run(req).await
}

/// Which rate bucket a request counts against, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Auth,
    Write,
}

fn classify(req: &Request) -> Option<Class> {
    let path = req.uri().path();
    if path.starts_with("/auth/") {
        return Some(Class::Auth);
    }
    if path.starts_with("/api/") && is_mutation(req) {
        return Some(Class::Write);
    }
    None
}

/// Global sliding-window rate limiter (shared across requests).
pub struct RateLimiter {
    auth_per_min: u32,
    write_per_min: u32,
    auth_hits: Mutex<Vec<Instant>>,
    write_hits: Mutex<Vec<Instant>>,
}

impl RateLimiter {
    /// Production floors: sign-in handshakes are rare, operator clicks many.
    pub fn defaults() -> Self {
        Self::new(60, 600)
    }

    /// Custom floors (tests).
    pub fn new(auth_per_min: u32, write_per_min: u32) -> Self {
        Self {
            auth_per_min,
            write_per_min,
            auth_hits: Mutex::new(Vec::new()),
            write_hits: Mutex::new(Vec::new()),
        }
    }

    /// Record a hit. `Ok` admits the request; `Err(retry_secs)` rejects it.
    fn check(&self, class: Class) -> Result<(), u64> {
        let (cap, hits) = match class {
            Class::Auth => (self.auth_per_min, &self.auth_hits),
            Class::Write => (self.write_per_min, &self.write_hits),
        };
        let now = Instant::now();
        let mut hits = hits.lock().expect("rate limiter lock");
        hits.retain(|t| now.duration_since(*t) < WINDOW);
        if hits.len() >= cap as usize {
            let oldest = hits.iter().min().copied().unwrap_or(now);
            let retry = WINDOW
                .checked_sub(now.duration_since(oldest))
                .map(|d| d.as_secs() + 1)
                .unwrap_or(1)
                .max(1);
            return Err(retry);
        }
        hits.push(now);
        Ok(())
    }
}

/// Enforce the global buckets. Over-limit requests get `429` + `Retry-After`.
pub async fn limit(State(limiter): State<Arc<RateLimiter>>, req: Request, next: Next) -> Response {
    if let Some(class) = classify(&req) {
        if let Err(retry) = limiter.check(class) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, retry.to_string())],
                Json(serde_json::json!({
                    "detail": format!("rate limit exceeded, retry in {retry}s")
                })),
            )
                .into_response();
        }
    }
    next.run(req).await
}

/// Cap handler futures so one wedged request cannot hold a connection forever.
pub async fn timeout(req: Request, next: Next) -> Response {
    match tokio::time::timeout(REQUEST_TIMEOUT, next.run(req)).await {
        Ok(resp) => resp,
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({ "detail": "request timed out" })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_admit_then_reject_then_recover() {
        let limiter = RateLimiter::new(2, 100);
        assert!(limiter.check(Class::Auth).is_ok());
        assert!(limiter.check(Class::Auth).is_ok());
        assert!(limiter.check(Class::Auth).is_err());
        // The write bucket is independent.
        assert!(limiter.check(Class::Write).is_ok());
    }

    #[test]
    fn classify_routes_auth_mutations_and_reads() {
        let req = Request::builder()
            .uri("/auth/login")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(classify(&req), Some(Class::Auth));
        let req = Request::builder()
            .uri("/auth/me")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(classify(&req), Some(Class::Auth));
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/orgs/default/apps")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(classify(&req), Some(Class::Write));
        let req = Request::builder()
            .uri("/api/v1/orgs/default/apps")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(classify(&req), None);
        let req = Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(classify(&req), None);
    }
}
