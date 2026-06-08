//! Router factory.
//!
//! [`create_router`] constructs the fully-configured [`axum::Router`], registers
//! all API routes, and layers on production-grade middleware.
//!
//! ## Middleware stack (outermost → innermost)
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │  SetRequestIdLayer  – stamps x-request-id UUID          │
//! │  ┌───────────────────────────────────────────────────┐  │
//! │  │  TraceLayer – structured HTTP span per request    │  │
//! │  │  ┌─────────────────────────────────────────────┐  │  │
//! │  │  │  Route handlers                             │  │  │
//! │  │  └─────────────────────────────────────────────┘  │  │
//! │  └───────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! Note: Axum applies layers from bottom to top in the builder chain, so
//! `SetRequestIdLayer` (added last) actually wraps `TraceLayer` (added first),
//! ensuring the request ID header is present when the span is opened.

use std::sync::Arc;

use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use tower_http::{
    request_id::{MakeRequestUuid, SetRequestIdLayer},
    trace::TraceLayer,
};

use crate::state::AppState;

pub mod chat;

// ─────────────────────────────────────────────────────────────────────────────

/// Assemble the full Axum [`Router`] with all routes and middleware.
///
/// The returned router is ready to be passed directly to [`axum::serve`].
pub fn create_router(state: Arc<AppState>) -> Router {
    // ── Routes ────────────────────────────────────────────────────────────────
    let api = Router::new()
        // Primary gateway endpoint – OpenAI-compatible chat completions.
        .route("/v1/chat/completions", post(chat::handler))
        // Liveness probe for container orchestrators (Kubernetes, Docker Swarm).
        .route("/health", get(health));

    // ── Middleware stack ──────────────────────────────────────────────────────
    Router::new()
        .merge(api)
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
                // Include the request ID in every span field so all log lines
                // for a single request share the same traceable identifier.
                let request_id = request
                    .headers()
                    .get("x-request-id")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("unknown");

                tracing::info_span!(
                    "http_request",
                    method     = %request.method(),
                    uri        = %request.uri(),
                    request_id,
                    // Placeholders recorded by TraceLayer on response.
                    status     = tracing::field::Empty,
                    latency_ms = tracing::field::Empty,
                )
            }),
        )
        // SetRequestIdLayer must wrap TraceLayer so the ID is stamped *before*
        // the span is opened (layer order is applied outermost → innermost).
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .with_state(state)
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Kubernetes/Docker liveness probe endpoint.
///
/// Returns `200 OK` with a plain-text body as long as the server is running.
async fn health() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}
