//! AI provider abstraction and fallback orchestration.
//!
//! This module owns the **fallback policy**: try OpenAI first; if it returns a
//! transient error (5xx or timeout), transparently retry via Anthropic.  Client
//! errors (4xx) are propagated immediately without attempting a fallback.
//!
//! ## Decision tree
//! ```text
//! client request
//!     │
//!     ▼
//! OpenAI ──success──▶ return response (cached by caller)
//!     │
//!     ├──retryable error (5xx / timeout)──▶ Anthropic ──success──▶ return response
//!     │                                         │
//!     │                                         └──any error──▶ AllProvidersFailed
//!     │
//!     └──non-retryable error (4xx)──▶ surface error to client immediately
//! ```

pub mod anthropic;
pub mod openai;

use serde_json::Value;

use crate::error::GatewayError;

// ─────────────────────────────────────────────────────────────────────────────

/// Attempt a chat completion, falling back from OpenAI → Anthropic on
/// transient provider failures (5xx HTTP status or network timeout).
///
/// Both provider functions receive the **original** OpenAI-format body.
/// `anthropic::complete` handles its own request translation internally.
///
/// # Parameters
/// - `request_body`     – parsed OpenAI `chat/completions` JSON payload.
/// - `http_client`      – shared connection-pooled HTTP client.
/// - `openai_api_key`   – `sk-…` key for OpenAI.
/// - `anthropic_api_key`– `sk-ant-…` key for Anthropic.
///
/// # Errors
/// Returns the last provider error wrapped in [`GatewayError::AllProvidersFailed`]
/// if both providers fail, or the OpenAI error directly for non-retryable cases.
pub async fn complete_with_fallback(
    request_body: &Value,
    http_client: &reqwest::Client,
    openai_api_key: &str,
    anthropic_api_key: &str,
) -> Result<Value, GatewayError> {
    // ── Primary: OpenAI ───────────────────────────────────────────────────────
    tracing::info!(provider = "openai", "Attempting primary provider");

    match openai::complete(request_body, http_client, openai_api_key).await {
        Ok(response) => {
            tracing::info!(provider = "openai", "Primary provider succeeded");
            return Ok(response);
        }
        Err(err) if err.is_retryable() => {
            tracing::warn!(
                provider = "openai",
                error    = %err,
                "Primary provider failed with a transient error – activating fallback"
            );
            // Fall through to Anthropic.
        }
        Err(err) => {
            // 4xx and internal errors are deterministic; a fallback won't help.
            tracing::error!(
                provider = "openai",
                error    = %err,
                "Primary provider failed with a non-retryable error"
            );
            return Err(err);
        }
    }

    // ── Fallback: Anthropic ───────────────────────────────────────────────────
    tracing::info!(provider = "anthropic", "Attempting fallback provider");

    match anthropic::complete(request_body, http_client, anthropic_api_key).await {
        Ok(response) => {
            tracing::info!(provider = "anthropic", "Fallback provider succeeded");
            Ok(response)
        }
        Err(err) => {
            tracing::error!(
                provider = "anthropic",
                error    = %err,
                "Fallback provider also failed – all providers exhausted"
            );
            Err(GatewayError::AllProvidersFailed(err.to_string()))
        }
    }
}
