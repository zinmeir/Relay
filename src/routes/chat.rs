//! Handler for `POST /v1/chat/completions`.
//!
//! This is the gateway's primary ingress point.  The request lifecycle is:
//!
//! ```text
//! 1. Buffer raw body bytes (capped at MAX_BODY_BYTES)
//! 2. Reject streaming requests (SSE not yet supported)
//! 3. Parse JSON → serde_json::Value
//! 4. Derive BLAKE3 cache key from raw bytes
//! 5. Cache HIT  → return Arc-cloned cached response (O(1), zero upstream call)
//! 6. Cache MISS → call providers::complete_with_fallback (OpenAI → Anthropic)
//! 7. Store Arc-wrapped response in cache
//! 8. Return response JSON to caller
//! ```

use std::sync::Arc;

use axum::{
    body,
    extract::{Request, State},
    response::Json,
};
use serde_json::Value;

use crate::{cache, error::GatewayError, providers, state::AppState};

// ─────────────────────────────────────────────────────────────────────────────

/// Maximum allowed request body size.
///
/// 10 MiB is generous even for the largest context windows currently offered
/// by OpenAI or Anthropic.  Callers exceeding this receive a 400 Bad Request.
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024; // 10 MiB

// ─────────────────────────────────────────────────────────────────────────────

/// Handle `POST /v1/chat/completions`.
///
/// Accepts any valid OpenAI chat-completions request body and returns a
/// standard OpenAI-format response JSON, either from cache or from an
/// upstream provider.
///
/// # Errors
/// All errors are converted to HTTP responses via [`GatewayError`]'s
/// [`axum::response::IntoResponse`] implementation.
pub async fn handler(
    State(state): State<Arc<AppState>>,
    // `Request` must be the *last* extractor – it consumes the full request body.
    request: Request,
) -> Result<Json<Value>, GatewayError> {
    // ── 1. Buffer raw body ────────────────────────────────────────────────────
    //
    // We need the raw bytes (not a pre-parsed JSON Value) so we can hash them
    // for the cache key *without* a round-trip through serialisation.
    let body_bytes = body::to_bytes(request.into_body(), MAX_BODY_BYTES)
        .await
        .map_err(|e| {
            GatewayError::RequestValidation(format!(
                "Failed to read request body (limit: {MAX_BODY_BYTES} bytes): {e}"
            ))
        })?;

    // ── 2. Guard against streaming requests ───────────────────────────────────
    //
    // Parse early enough to check the `stream` flag; a full parse is required
    // anyway (step 3), so there is no double-parsing cost.
    let request_json: Value = serde_json::from_slice(&body_bytes)?;

    if request_json.get("stream").and_then(Value::as_bool) == Some(true) {
        return Err(GatewayError::RequestValidation(
            "Streaming (SSE) is not supported by this gateway. \
             Remove `\"stream\": true` from your request body."
                .to_owned(),
        ));
    }

    // ── 3. Derive cache key ───────────────────────────────────────────────────
    let cache_key = cache::derive_key(&body_bytes);

    // ── 4. Cache lookup ───────────────────────────────────────────────────────
    if let Some(cached) = state.cache.get(&cache_key).await {
        tracing::info!(
            cache_key = %cache_key,
            "Cache HIT – returning cached response without upstream call"
        );
        // Deref the Arc and clone the underlying Value for the response body.
        // The Arc itself stays alive in the cache; this clone is unavoidable
        // but is the only allocation on the hot (cache-hit) path.
        return Ok(Json((*cached).clone()));
    }

    tracing::info!(
        cache_key = %cache_key,
        "Cache MISS – forwarding to provider"
    );

    // ── 5. Provider call with fallback ────────────────────────────────────────
    let response = providers::complete_with_fallback(
        &request_json,
        &state.http_client,
        &state.config.openai_api_key,
        &state.config.anthropic_api_key,
    )
    .await?;

    // ── 6. Populate cache ─────────────────────────────────────────────────────
    //
    // Clone the response once: one copy goes into the Arc for the cache,
    // the original is moved into the Json wrapper for the HTTP response.
    // Future cache hits will Arc-clone (O(1)) instead of repeating the clone.
    state
        .cache
        .insert(cache_key.clone(), Arc::new(response.clone()))
        .await;

    tracing::debug!(cache_key = %cache_key, "Response inserted into cache");

    Ok(Json(response))
}
