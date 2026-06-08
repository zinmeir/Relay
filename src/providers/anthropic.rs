//! Anthropic Messages-API provider.
//!
//! This module handles the **bidirectional schema translation** needed to use
//! Anthropic as a drop-in fallback for an OpenAI-compatible gateway:
//!
//! 1. [`convert_request`] – rewrites an OpenAI `chat/completions` body into
//!    Anthropic's `messages` format (model swap, system-prompt extraction,
//!    field mapping).
//! 2. [`complete`] – POSTs the translated request to Anthropic and calls
//!    [`convert_response`] on the result.
//! 3. [`convert_response`] – normalises Anthropic's response back into the
//!    OpenAI `chat.completion` schema so the rest of the gateway is unaware
//!    that a fallback occurred.

use serde_json::{json, Value};

use crate::error::GatewayError;

// ─────────────────────────────────────────────────────────────────────────────

const ANTHROPIC_API_URL: &str  = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str  = "2023-06-01";
/// The Claude model used for all fallback completions.
const FALLBACK_MODEL: &str     = "claude-3-5-sonnet-20241022";
/// Applied when the OpenAI request omits `max_tokens` (Anthropic requires it).
const DEFAULT_MAX_TOKENS: u64  = 1_024;

// ─────────────────────────────────────────────────────────────────────────────

/// Send an OpenAI-format chat-completion request to Anthropic's Messages API.
///
/// Internally this calls [`convert_request`], POSTs to Anthropic, then calls
/// [`convert_response`] so the caller receives a standard OpenAI-shaped JSON
/// object regardless of which provider actually served the request.
///
/// # Errors
/// - [`GatewayError::RequestValidation`] – the source request cannot be translated.
/// - [`GatewayError::ProviderTimeout`]   – the request exceeded the client timeout.
/// - [`GatewayError::ProviderHttpError`] – Anthropic returned a non-2xx status.
/// - [`GatewayError::Internal`]          – network or JSON parsing failure.
#[tracing::instrument(name = "anthropic.complete", skip_all, fields(provider = "anthropic"))]
pub async fn complete(
    body: &Value,
    client: &reqwest::Client,
    api_key: &str,
) -> Result<Value, GatewayError> {
    let anthropic_body = convert_request(body)?;

    tracing::debug!(
        url   = ANTHROPIC_API_URL,
        model = FALLBACK_MODEL,
        "Sending request to Anthropic"
    );

    let http_response = client
        .post(ANTHROPIC_API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&anthropic_body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                tracing::warn!("Anthropic request timed out");
                GatewayError::ProviderTimeout
            } else {
                tracing::error!(error = %e, "Anthropic network error");
                GatewayError::Internal(anyhow::anyhow!("Anthropic network error: {e}"))
            }
        })?;

    let status = http_response.status();

    if !status.is_success() {
        let status_code = status.as_u16();
        let error_body = http_response.text().await.unwrap_or_default();
        tracing::warn!(
            http_status = status_code,
            body = %error_body,
            "Anthropic returned an error status"
        );
        return Err(GatewayError::ProviderHttpError {
            status: status_code,
            body: error_body,
        });
    }

    let anthropic_response = http_response.json::<Value>().await.map_err(|e| {
        GatewayError::Internal(anyhow::anyhow!(
            "Failed to deserialise Anthropic response body: {e}"
        ))
    })?;

    tracing::debug!("Anthropic request completed successfully");
    Ok(convert_response(anthropic_response))
}

// ─────────────────────────────────────────────────────────────────────────────
// Schema translation – OpenAI → Anthropic
// ─────────────────────────────────────────────────────────────────────────────

/// Translate an OpenAI `chat/completions` request body into Anthropic's
/// `messages` API format.
///
/// ## Field mapping
/// | OpenAI field        | Anthropic field      | Notes                                        |
/// |---------------------|----------------------|----------------------------------------------|
/// | `messages[].role`   | `messages[].role`    | system messages are lifted to `system`        |
/// | `messages[].content`| `messages[].content` | kept as a plain string                       |
/// | `model`             | `model`              | **always overridden** to [`FALLBACK_MODEL`]  |
/// | `max_tokens`        | `max_tokens`         | defaults to [`DEFAULT_MAX_TOKENS`] if absent |
/// | `temperature`       | `temperature`        | passed through unchanged                     |
///
/// # Errors
/// Returns [`GatewayError::RequestValidation`] if the `messages` field is
/// missing or if there are no non-system messages to send.
fn convert_request(openai_body: &Value) -> Result<Value, GatewayError> {
    let messages = openai_body["messages"].as_array().ok_or_else(|| {
        GatewayError::RequestValidation(
            "Request is missing a valid `messages` array".to_owned(),
        )
    })?;

    // ── Lift system messages into a top-level `system` field ──────────────────
    // OpenAI allows system role messages inside the array; Anthropic expects
    // them as a single top-level string.  Multiple system messages are joined.
    let system_text: String = messages
        .iter()
        .filter(|m| m["role"].as_str() == Some("system"))
        .filter_map(|m| m["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n");

    // ── Filter out system messages from the main array ────────────────────────
    let converted_messages: Vec<Value> = messages
        .iter()
        .filter(|m| m["role"].as_str() != Some("system"))
        .map(|m| {
            json!({
                "role":    m["role"].as_str().unwrap_or("user"),
                "content": m["content"].as_str().unwrap_or(""),
            })
        })
        .collect();

    if converted_messages.is_empty() {
        return Err(GatewayError::RequestValidation(
            "No non-system messages found; Anthropic requires at least one user/assistant message"
                .to_owned(),
        ));
    }

    let max_tokens = openai_body["max_tokens"]
        .as_u64()
        .unwrap_or(DEFAULT_MAX_TOKENS);

    let mut anthropic_body = json!({
        "model":      FALLBACK_MODEL,
        "max_tokens": max_tokens,
        "messages":   converted_messages,
    });

    if !system_text.is_empty() {
        anthropic_body["system"] = Value::String(system_text);
    }

    // Pass temperature through if present and finite.
    if let Some(temp) = openai_body["temperature"].as_f64() {
        if let Some(num) = serde_json::Number::from_f64(temp) {
            anthropic_body["temperature"] = Value::Number(num);
        }
    }

    Ok(anthropic_body)
}

// ─────────────────────────────────────────────────────────────────────────────
// Schema translation – Anthropic → OpenAI
// ─────────────────────────────────────────────────────────────────────────────

/// Normalise an Anthropic Messages API response into the OpenAI
/// `chat.completion` schema.
///
/// This is a best-effort, lossy translation: Anthropic-specific fields
/// (citations, tool-use blocks, etc.) are silently dropped.  Only the first
/// `text` content block is surfaced as the assistant message.
///
/// ## Field mapping
/// | Anthropic field      | OpenAI field                  |
/// |----------------------|-------------------------------|
/// | `id`                 | `id` (prefixed `chatcmpl-`)   |
/// | `model`              | `model`                       |
/// | `content[0].text`    | `choices[0].message.content`  |
/// | `stop_reason`        | `choices[0].finish_reason`    |
/// | `usage.input_tokens` | `usage.prompt_tokens`         |
/// | `usage.output_tokens`| `usage.completion_tokens`     |
fn convert_response(anthropic: Value) -> Value {
    // Extract the first text content block (best-effort; empty string if absent).
    let content_text: &str = anthropic["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| {
            if block["type"].as_str() == Some("text") {
                block["text"].as_str()
            } else {
                None
            }
        })
        .unwrap_or_default();

    let model = anthropic["model"].as_str().unwrap_or(FALLBACK_MODEL);
    let msg_id = anthropic["id"].as_str().unwrap_or("unknown");

    let input_tokens  = anthropic["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output_tokens = anthropic["usage"]["output_tokens"].as_u64().unwrap_or(0);

    // Map Anthropic stop reasons to OpenAI finish reasons.
    let finish_reason = match anthropic["stop_reason"].as_str() {
        Some("end_turn")      => "stop",
        Some("max_tokens")    => "length",
        Some("stop_sequence") => "stop",
        _                     => "stop",
    };

    let created_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    json!({
        "id":      format!("chatcmpl-{msg_id}"),
        "object":  "chat.completion",
        "created": created_unix,
        "model":   model,
        "choices": [{
            "index":         0,
            "message": {
                "role":    "assistant",
                "content": content_text,
            },
            "logprobs":      null,
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens":     input_tokens,
            "completion_tokens": output_tokens,
            "total_tokens":      input_tokens + output_tokens,
        }
    })
}
