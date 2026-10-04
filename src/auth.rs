//! GitHub OAuth (authorization-code) with an allowlist and HttpOnly sessions.
//!
//! Mirrors the fleet's existing pattern (monthly-logs): only numeric GitHub
//! user ids on `allowed_github_ids` may sign in. Debug builds may bypass the
//! whole flow with `AUTH_DISABLED=1`.

use axum::extract::{Query, State};
use axum::http::header::{ACCEPT, USER_AGENT};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};

use turaes_core::crypto::random_token;
use turaes_core::{Error, Result};

use crate::state::AppState;

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "turaes_session";
const STATE_COOKIE: &str = "turaes_oauth_state";

const GITHUB_AUTHORIZE: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER: &str = "https://api.github.com/user";

/// Authenticated principal attached to request extensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthUser {
    /// GitHub numeric id (`0` for the dev user).
    pub id: i64,
    /// GitHub login.
    pub login: String,
    /// Display name, when known.
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    id: i64,
    login: String,
    name: Option<String>,
}

fn session_cookie(state: &AppState, value: String) -> Cookie<'static> {
    let mut cookie = Cookie::new(SESSION_COOKIE, value);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path("/");
    cookie.set_secure(state.cfg.secure_cookies());
    cookie
}

/// A cookie that clears `name`. Must mirror the identity attributes used when
/// setting it (notably `Path`), or the browser won't match/evict the original.
fn removal_cookie(state: &AppState, name: &str) -> Cookie<'static> {
    let mut cookie = Cookie::new(name.to_string(), "");
    cookie.set_path("/");
    cookie.set_same_site(SameSite::Lax);
    cookie.set_secure(state.cfg.secure_cookies());
    cookie.make_removal();
    cookie
}

/// Redirect back to the dashboard with a user-facing sign-in error, clearing
/// the transient OAuth state cookie.
fn login_error(state: &AppState, jar: CookieJar, msg: &str) -> Response {
    let jar = jar.add(removal_cookie(state, STATE_COOKIE));
    let location = format!("/?login_error={}", urlencoding::encode(msg));
    (jar, Redirect::temporary(&location)).into_response()
}

fn state_cookie(value: String) -> Cookie<'static> {
    let mut cookie = Cookie::new(STATE_COOKIE, value);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path("/");
    cookie
}

fn user_from_jar(state: &AppState, jar: &CookieJar) -> Result<AuthUser> {
    if state.auth_disabled {
        return Ok(dev_user());
    }
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or_else(|| Error::Unauthorized("no session (sign in at /auth/login)".into()))?;
    let claims = state.issuer.verify(&token)?;
    Ok(AuthUser {
        id: claims.sub.parse().unwrap_or(0),
        login: claims.login,
        name: claims.name,
    })
}

fn dev_user() -> AuthUser {
    AuthUser {
        id: 0,
        login: "dev".into(),
        name: Some("Dev mode".into()),
    }
}

/// `GET /auth/login` — start the OAuth dance.
pub async fn login(State(state): State<AppState>, jar: CookieJar) -> Result<Response> {
    if state.auth_disabled {
        return Ok(Redirect::temporary("/").into_response());
    }
    if state.cfg.auth.github_client_id.is_empty() {
        return Err(Error::Config(
            "GitHub OAuth is not configured (set TURAES_GITHUB_CLIENT_ID)".into(),
        ));
    }
    // Already signed in: no need to start another OAuth round-trip.
    if user_from_jar(&state, &jar).is_ok() {
        return Ok(Redirect::temporary("/").into_response());
    }
    let nonce = random_token(24);
    let callback = state.cfg.callback_url();
    let url = format!(
        "{GITHUB_AUTHORIZE}?client_id={}&redirect_uri={}&scope=read:user&state={}",
        urlencoding::encode(&state.cfg.auth.github_client_id),
        urlencoding::encode(&callback),
        urlencoding::encode(&nonce),
    );
    let jar = jar.add(state_cookie(nonce));
    Ok((jar, Redirect::temporary(&url)).into_response())
}

/// `GET /auth/callback` — exchange the code, enforce the allowlist, set session.
pub async fn callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(q): Query<CallbackQuery>,
) -> Result<Response> {
    if let Some(err) = q.error {
        let msg = match q.error_description.as_deref() {
            Some(d) if !d.is_empty() => format!("GitHub sign-in was not completed ({err}: {d})"),
            _ => format!("GitHub sign-in was not completed ({err})"),
        };
        return Ok(login_error(&state, jar.clone(), &msg));
    }
    let expected = match jar.get(STATE_COOKIE) {
        Some(c) => c.value().to_string(),
        None => {
            return Ok(login_error(
                &state,
                jar,
                "Sign-in session expired; please try again",
            ))
        }
    };
    let got = match q.state {
        Some(s) => s,
        None => return Ok(login_error(&state, jar.clone(), "Invalid sign-in response")),
    };
    if expected != got {
        return Ok(login_error(
            &state,
            jar.clone(),
            "Sign-in state mismatch; please try again",
        ));
    }
    let code = match q.code {
        Some(c) => c,
        None => return Ok(login_error(&state, jar.clone(), "Missing sign-in code")),
    };

    let token_resp = state
        .http
        .post(GITHUB_TOKEN)
        .header(ACCEPT, "application/json")
        .form(&[
            ("client_id", state.cfg.auth.github_client_id.as_str()),
            (
                "client_secret",
                state.cfg.auth.github_client_secret.as_str(),
            ),
            ("code", code.as_str()),
            ("redirect_uri", state.cfg.callback_url().as_str()),
        ])
        .send()
        .await;
    let token: serde_json::Value = match token_resp {
        Ok(resp) => match resp.json().await {
            Ok(v) => v,
            Err(_) => return Ok(login_error(&state, jar, "Could not read GitHub's response")),
        },
        Err(_) => {
            return Ok(login_error(
                &state,
                jar,
                "Could not reach GitHub to complete sign-in",
            ))
        }
    };
    let access_token = match token.get("access_token").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return Ok(login_error(&state, jar, "GitHub did not grant access")),
    };

    let user_resp = state
        .http
        .get(GITHUB_USER)
        .header(USER_AGENT, "turaes")
        .bearer_auth(access_token)
        .send()
        .await;
    let user: GhUser = match user_resp {
        Ok(resp) => match resp.json().await {
            Ok(u) => u,
            Err(_) => {
                return Ok(login_error(
                    &state,
                    jar,
                    "Could not read your GitHub profile",
                ))
            }
        },
        Err(_) => return Ok(login_error(&state, jar, "Could not reach GitHub")),
    };

    if !state.cfg.auth.allowed_github_ids.is_empty()
        && !state.cfg.auth.allowed_github_ids.contains(&user.id)
    {
        return Ok(login_error(
            &state,
            jar,
            &format!("@{} is not allowed to sign in", user.login),
        ));
    }

    // Persist the user row (and bootstrap their membership) at sign-in so
    // the tenant principal exists before the first API request.
    crate::authz::resolve(&state, user.id, &user.login, user.name.as_deref()).await?;

    let token = state
        .issuer
        .mint(user.id, &user.login, user.name.as_deref())?;
    let jar = jar
        .add(session_cookie(&state, token))
        .add(removal_cookie(&state, STATE_COOKIE));
    Ok((jar, Redirect::temporary("/")).into_response())
}

/// `GET /auth/me` — current user (protected by the auth layer).
pub async fn me(State(state): State<AppState>, jar: CookieJar) -> Result<Json<AuthUser>> {
    Ok(Json(user_from_jar(&state, &jar)?))
}

/// `POST /auth/logout` — clear the session cookie.
pub async fn logout(State(state): State<AppState>, jar: CookieJar) -> Response {
    let jar = jar.add(removal_cookie(&state, SESSION_COOKIE));
    (jar, StatusCode::NO_CONTENT).into_response()
}
/// Extract a `Bearer` API token from the `Authorization` header, if present.
fn bearer_token(req: &axum::extract::Request) -> Option<String> {
    let value = req.headers().get(axum::http::header::AUTHORIZATION)?;
    let value = value.to_str().ok()?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?;
    if token.trim().is_empty() {
        return None;
    }
    Some(token.to_string())
}

/// Middleware: reject unauthenticated requests, and attach both the OAuth
/// principal (`AuthUser`) and the resolved tenant principal (`CurrentUser`).
/// An explicit `Authorization: Bearer` API token always wins over the session
/// cookie (and over the dev bypass, so CI-style access is testable).
pub async fn require_auth(
    State(state): State<AppState>,
    mut req: axum::extract::Request,
    next: Next,
) -> Result<Response> {
    if let Some(token) = bearer_token(&req) {
        let current = crate::authz::resolve_token(&state, &token).await?;
        req.extensions_mut().insert(current);
        return Ok(next.run(req).await);
    }
    if state.auth_disabled {
        let current = crate::authz::resolve(&state, 0, "dev", Some("Dev mode")).await?;
        req.extensions_mut().insert(dev_user());
        req.extensions_mut().insert(current);
        return Ok(next.run(req).await);
    }
    let jar = CookieJar::from_headers(req.headers());
    let user = user_from_jar(&state, &jar)?;
    let current = crate::authz::resolve(&state, user.id, &user.login, user.name.as_deref()).await?;
    req.extensions_mut().insert(user);
    req.extensions_mut().insert(current);
    Ok(next.run(req).await)
}
