//! The single error type crossing turaes boundaries.
//!
//! Errors map to a JSON body with a stable machine-readable `code`; validation
//! responses may also include `field` so forms can put feedback next to input.
//! Bad input is 422 (not 400) to stay consistent across the fleet.

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
    /// Input failed validation for a specific request field.
    #[error("invalid input: {detail}")]
    FieldValidation { field: String, detail: String },
    /// The request conflicts with current state.
    #[error("conflict: {0}")]
    Conflict(String),
    /// The request conflicts with current state for a specific field.
    #[error("conflict: {detail}")]
    FieldConflict { field: String, detail: String },
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
            Error::BadRequest(_) | Error::FieldValidation { .. } => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            Error::Conflict(_) | Error::FieldConflict { .. } => StatusCode::CONFLICT,
            Error::Config(_) | Error::Db(_) | Error::Io(_) | Error::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    /// Stable machine-readable code for the variant. Clients should switch on
    /// this, never on the human `detail` text.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Config(_) => "config",
            Error::NotFound(_) => "not_found",
            Error::Unauthorized(_) => "unauthorized",
            Error::Forbidden(_) => "forbidden",
            Error::BadRequest(_) | Error::FieldValidation { .. } => "bad_request",
            Error::Conflict(_) | Error::FieldConflict { .. } => "conflict",
            Error::Db(_) => "db",
            Error::Io(_) => "io",
            Error::Internal(_) => "internal",
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        let body = match &self {
            Error::FieldValidation { field, .. } | Error::FieldConflict { field, .. } => {
                json!({ "detail": self.to_string(), "code": self.code(), "field": field })
            }
            _ => json!({ "detail": self.to_string(), "code": self.code() }),
        };
        (status, Json(body)).into_response()
    }
}
