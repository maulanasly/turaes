//! The single error type crossing turaes boundaries.
//!
//! Every variant maps to a JSON body `{"detail": "..."}` — the same shape
//! beruang and monthly-logs use. Bad input is 422 (not 400) to stay consistent
//! across the fleet.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Result alias used throughout turaes.
pub type Result<T> = std::result::Result<T, Error>;

/// Domain error for turaes.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Configuration is missing or invalid (boot-time).
    #[error("configuration error: {0}")]
    Config(String),
    /// A requested entity does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The caller is not authenticated.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// The caller is authenticated but not permitted.
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// Input failed validation.
    #[error("invalid input: {0}")]
    BadRequest(String),
    /// The request conflicts with current state.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A storage layer failure.
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    /// A wrapped I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// An unexpected internal failure.
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// HTTP status this error renders as.
    pub fn status(&self) -> StatusCode {
        match self {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Error::Forbidden(_) => StatusCode::FORBIDDEN,
            Error::BadRequest(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::Config(_) | Error::Db(_) | Error::Io(_) | Error::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        (status, Json(json!({ "detail": self.to_string() }))).into_response()
    }
}
