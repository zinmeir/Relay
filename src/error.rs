//! Gateway-wide error types.
//!
//! [`GatewayError`] is the single error enum returned by every Axum handler.
//! Implementing [`axum::response::IntoResponse`] lets handlers use the ergonomic
//! `Result<_, GatewayError>` return type; errors are automatically converted into
//! well-formed HTTP JSON responses with appropriate status codes.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

// ─────────────────────────────────────────────────────────────────────────────

/// Top-level error type for every fallible operation in the LLM Gateway.
#[derive(Debug, Error)]
pub enum GatewayError {
    // ── Provider errors ───────────────────────────────────────────────────────

    /// Both the primary (OpenAI) and fallback (Anthropic) providers failed.
    #[error("All upstream providers failed: {0}")]
    AllProvidersFailed(String),

    /// A provider returned an HTTP error status.
    #[error("Provider returned HTTP {status}: {body}")]
    ProviderHttpError { status: u16, body: String },

    /// A provider request timed out before returning a response.
    #[error("Provider request timed out")]
    ProviderTimeout,

    // ── Request errors ────────────────────────────────────────────────────────

    /// The incoming request body could not be deserialised as valid JSON.
    #[error("Invalid request body: {0}")]
    InvalidRequestBody(#[from] serde_json::Error),

    /// The incoming request is structurally invalid (missing fields, bad values,
    /// unsupported features such as streaming, etc.).
    #[error("Request validation failed: {0}")]
    RequestValidation(String),

    // ── Internal errors ───────────────────────────────────────────────────────

    /// Catch-all for unexpected internal failures.  Wraps [`anyhow::Error`] so
    /// that any error enriched with `.context(…)` call-chains can surface here.
    /// The detail is intentionally **not** forwarded to the HTTP client.
    #[error("Internal gateway error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl GatewayError {
    /// Returns `true` if this error represents a *transient* provider failure
    /// (5xx HTTP status or network timeout) that warrants trying a fallback provider.
    ///
    /// Client errors (4xx) and internal errors are **not** retryable.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::ProviderTimeout => true,
            Self::ProviderHttpError { status, .. } => *status >= 500,
            _ => false,
        }
    }
}

// ── axum IntoResponse ────────────────────────────────────────────────────────

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, client_message, is_server_fault) = match &self {
            GatewayError::AllProvidersFailed(_) => {
                (StatusCode::BAD_GATEWAY, self.to_string(), true)
            }
            GatewayError::ProviderHttpError { .. } => {
                (StatusCode::BAD_GATEWAY, self.to_string(), true)
            }
            GatewayError::ProviderTimeout => {
                (StatusCode::GATEWAY_TIMEOUT, self.to_string(), true)
            }
            GatewayError::InvalidRequestBody(_) => {
                (StatusCode::BAD_REQUEST, self.to_string(), false)
            }
            GatewayError::RequestValidation(_) => {
                (StatusCode::BAD_REQUEST, self.to_string(), false)
            }
            GatewayError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                // Never expose internal detail to callers.
                "An unexpected internal error occurred. Please try again.".to_owned(),
                true,
            ),
        };

        // Log at the right severity: server faults are errors, client mistakes
        // are warnings (they're expected traffic in normal operation).
        if is_server_fault {
            tracing::error!(error = %self, http_status = %status, "Server-side error");
        } else {
            tracing::warn!(error = %self, http_status = %status, "Client error");
        }

        let body = Json(json!({
            "error": {
                "message": client_message,
                "type":    status.canonical_reason().unwrap_or("error"),
            }
        }));

        (status, body).into_response()
    }
}
