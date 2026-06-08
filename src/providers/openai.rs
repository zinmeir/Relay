//! OpenAI chat-completions provider.
//!
//! Forwards a standard OpenAI-format request to `https://api.openai.com/v1/chat/completions`
//! and returns the response JSON verbatim.  No schema translation is needed
//! because the gateway speaks OpenAI's format natively.

use serde_json::Value;

use crate::error::GatewayError;

// ─────────────────────────────────────────────────────────────────────────────

const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";

// ─────────────────────────────────────────────────────────────────────────────

/// Send an OpenAI-format chat-completion request to OpenAI's API.
///
/// The `body` is forwarded as-is (after bearer-auth is attached), so any
/// valid OpenAI parameters (temperature, top_p, tools, etc.) are passed
/// through transparently.
///
/// # Errors
/// - [`GatewayError::ProviderTimeout`] – the request exceeded the client timeout.
/// - [`GatewayError::ProviderHttpError`] – OpenAI returned a non-2xx status.
/// - [`GatewayError::Internal`] – network failure or response body parsing error.
#[tracing::instrument(name = "openai.complete", skip_all, fields(provider = "openai"))]
pub async fn complete(
    body: &Value,
    client: &reqwest::Client,
    api_key: &str,
) -> Result<Value, GatewayError> {
    tracing::debug!(url = OPENAI_API_URL, "Sending request to OpenAI");

    let http_response = client
        .post(OPENAI_API_URL)
        .bearer_auth(api_key)
        .json(body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                tracing::warn!("OpenAI request timed out");
                GatewayError::ProviderTimeout
            } else {
                tracing::error!(error = %e, "OpenAI network error");
                GatewayError::Internal(anyhow::anyhow!("OpenAI network error: {e}"))
            }
        })?;

    let status = http_response.status();

    if !status.is_success() {
        let status_code = status.as_u16();
        // Read the error body for diagnostics; tolerate failure (empty string).
        let error_body = http_response.text().await.unwrap_or_default();
        tracing::warn!(
            http_status = status_code,
            body = %error_body,
            "OpenAI returned an error status"
        );
        return Err(GatewayError::ProviderHttpError {
            status: status_code,
            body: error_body,
        });
    }

    let response_json = http_response.json::<Value>().await.map_err(|e| {
        GatewayError::Internal(anyhow::anyhow!(
            "Failed to deserialise OpenAI response body: {e}"
        ))
    })?;

    tracing::debug!("OpenAI request completed successfully");
    Ok(response_json)
}
