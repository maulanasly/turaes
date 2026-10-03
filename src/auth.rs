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
        .ok_or_else(|| Error::Unauthorized("no session".into()))?;
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
        return Err(Error::Unauthorized(format!(
            "GitHub OAuth error: {err} {}",
            q.error_description.unwrap_or_default()
        )));
    }
    let expected = jar
        .get(STATE_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or_else(|| Error::Unauthorized("missing OAuth state cookie".into()))?;
    let got = q
        .state
        .ok_or_else(|| Error::BadRequest("missing OAuth state".into()))?;
    if expected != got {
        return Err(Error::Unauthorized("OAuth state mismatch".into()));
    }
    let code = q
        .code
        .ok_or_else(|| Error::BadRequest("missing OAuth code".into()))?;

    let token: serde_json::Value = state
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
        .await
        .map_err(|e| Error::Internal(format!("GitHub token exchange failed: {e}")))?
        .json()
        .await
        .map_err(|e| Error::Internal(format!("GitHub token response invalid: {e}")))?;
    let access_token = token
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Unauthorized("GitHub did not return an access token".into()))?;

    let user: GhUser = state
        .http
        .get(GITHUB_USER)
        .header(USER_AGENT, "turaes")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| Error::Internal(format!("GitHub user fetch failed: {e}")))?
        .json()
        .await
        .map_err(|e| Error::Internal(format!("GitHub user response invalid: {e}")))?;

    if !state.cfg.auth.allowed_github_ids.is_empty()
        && !state.cfg.auth.allowed_github_ids.contains(&user.id)
    {
        return Err(Error::Forbidden(format!(
            "GitHub user {} is not on the allowlist",
            user.login
        )));
    }

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

/// Middleware: reject unauthenticated requests, or attach the dev user.
pub async fn require_auth(
    State(state): State<AppState>,
    mut req: axum::extract::Request,
    next: Next,
) -> Result<Response> {
    if state.auth_disabled {
        req.extensions_mut().insert(dev_user());
        return Ok(next.run(req).await);
    }
    let jar = CookieJar::from_headers(req.headers());
    let user = user_from_jar(&state, &jar)?;
    req.extensions_mut().insert(user);
    Ok(next.run(req).await)
}
