//! # LLM Gateway – Entry Point
//!
//! Bootstraps the multi-threaded Tokio runtime, structured tracing,
//! the in-memory response cache, the shared HTTP client, and the Axum HTTP
//! server.  Wires everything together through [`AppState`] and hands off to
//! [`routes::create_router`].
//!
//! ## Startup sequence
//! 1. Load `.env` file (no-op if absent – safe for production).
//! 2. Initialise `tracing-subscriber` with an `EnvFilter` driven by `RUST_LOG`.
//! 3. Parse [`config::Config`] from environment variables (fails fast if
//!    required keys are missing).
//! 4. Build the shared [`reqwest::Client`] connection pool.
//! 5. Construct the [`moka::future::Cache`] with capacity and TTL limits.
//! 6. Assemble [`state::AppState`] and wrap it in [`std::sync::Arc`].
//! 7. Build the Axum router via [`routes::create_router`].
//! 8. Bind a TCP listener and serve with graceful shutdown on SIGINT / SIGTERM.

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use moka::future::Cache;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod cache;
mod config;
mod error;
mod providers;
mod routes;
mod state;

use state::{AppState, CachedResponse};

// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── 0. Load .env (development convenience; no-op in production) ──────────
    //
    // dotenvy::dotenv() returns Err if the file is not found.  We intentionally
    // discard that error – in production, variables are injected by the runtime
    // environment and no .env file is expected.
    dotenvy::dotenv().ok();

    // ── 1. Structured, levelled tracing ─────────────────────────────────────
    //
    // The filter is sourced from the `RUST_LOG` environment variable.  If the
    // variable is absent or malformed we fall back to a sensible default that
    // surfaces all gateway-internal spans at DEBUG level while keeping noisy
    // third-party crates at INFO.
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "llm_gateway=debug,tower_http=debug,moka=info".into()
            }),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .compact()          // single-line format – readable in a terminal
                .with_target(true)  // include the module path in every log line
                .with_level(true),
        )
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "LLM Gateway starting up"
    );

    // ── 2. Configuration ─────────────────────────────────────────────────────
    //
    // Any missing required variable or malformed optional variable causes an
    // immediate, descriptive error – fail fast rather than silently misbehave.
    let config = config::Config::from_env()
        .context("Failed to load configuration from environment variables")?;

    // Log the config at DEBUG; the custom Debug impl redacts API keys.
    tracing::debug!(?config, "Configuration loaded");

    // ── 3. Shared reqwest HTTP client ────────────────────────────────────────
    //
    // A single `reqwest::Client` is shared across all handler tasks.  The
    // client manages an internal connection pool, so sharing avoids redundant
    // TCP / TLS handshakes and keeps the open-file-descriptor count bounded.
    let http_client = reqwest::Client::builder()
        // Provider-level deadline.  Individual calls may impose stricter limits
        // via `tokio::time::timeout` where needed.
        .timeout(Duration::from_secs(config.provider_timeout_secs))
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION"),
        ))
        .build()
        .context("Failed to construct the shared HTTP client")?;

    tracing::debug!("Shared HTTP client ready (connection pool initialised)");

    // ── 4. In-memory response cache ──────────────────────────────────────────
    //
    // Keys  : hex-encoded BLAKE3 hash of the canonical request body (64 chars).
    // Values: `Arc<serde_json::Value>` – cheaply cloneable into handlers.
    //
    // `moka::future::Cache` is internally Arc-backed; cloning the handle below
    // into `AppState` is O(1) and all clones share the same underlying storage.
    let response_cache: Cache<String, CachedResponse> = Cache::builder()
        .max_capacity(config.cache_max_capacity)
        .time_to_live(Duration::from_secs(config.cache_ttl_secs))
        // An eviction listener lets us track cache churn via tracing.
        // The closure must be Send + Sync + 'static; it runs synchronously on
        // the evicting thread, so keep it cheap (a single tracing call is fine).
        .eviction_listener(|key, _value, cause| {
            tracing::trace!(cache_key = %key, ?cause, "Cache entry evicted");
        })
        .build();

    tracing::info!(
        max_capacity = config.cache_max_capacity,
        ttl_secs     = config.cache_ttl_secs,
        "In-memory response cache ready"
    );

    // ── 5. Application state ─────────────────────────────────────────────────
    //
    // `Arc` lets Axum clone the state handle into each spawned handler task
    // in O(1); the underlying Config / client / cache are shared, not copied.
    let state = Arc::new(AppState::new(config, http_client, response_cache));

    // ── 6. Router ────────────────────────────────────────────────────────────
    let app = routes::create_router(Arc::clone(&state));

    // ── 7. Bind TCP listener ─────────────────────────────────────────────────
    let bind_addr = format!("0.0.0.0:{}", state.config.port);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("Failed to bind TCP listener on {bind_addr}"))?;

    let local_addr = listener
        .local_addr()
        .context("Failed to read the bound socket address")?;

    tracing::info!(%local_addr, "HTTP server ready – awaiting connections");

    // ── 8. Serve with graceful shutdown ──────────────────────────────────────
    //
    // `with_graceful_shutdown` resolves the provided future to signal that no
    // new connections should be accepted; Axum then drains in-flight requests
    // before `serve` returns.
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("Axum server encountered a fatal error")?;

    tracing::info!("Server shut down gracefully – goodbye!");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────

/// Resolves when **CTRL+C** (SIGINT) or **SIGTERM** is received.
///
/// Passing this future to [`axum::serve::Serve::with_graceful_shutdown`] gives
/// the server time to drain in-flight requests before the process exits.
///
/// On non-Unix platforms (e.g. Windows) only CTRL+C is handled; SIGTERM is
/// approximated by a never-resolving future that effectively disables it.
async fn shutdown_signal() {
    // SIGINT – triggered by Ctrl+C in a terminal or `kill -INT <pid>`.
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            // If we cannot register the handler we log it, but we still return
            // from this branch so `select!` can proceed (worst case: no graceful
            // shutdown on SIGINT, but the process remains alive and healthy).
            tracing::error!(%err, "Failed to register CTRL+C signal handler");
        }
    };

    // SIGTERM – sent by container orchestrators (Kubernetes, Docker) during
    // graceful pod termination.  Unix only.
    #[cfg(unix)]
    let sigterm = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                // `recv()` resolves when the first SIGTERM arrives.
                stream.recv().await;
            }
            Err(err) => {
                // Installation failure is non-fatal; log and keep the branch
                // pending so only CTRL+C can trigger a graceful shutdown.
                tracing::warn!(
                    %err,
                    "Could not install SIGTERM handler; \
                     only CTRL+C will trigger graceful shutdown"
                );
                std::future::pending::<()>().await;
            }
        }
    };

    // On Windows or other non-Unix targets, SIGTERM is not a POSIX concept;
    // use a never-resolving future as a no-op placeholder.
    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c  => tracing::info!("Received CTRL+C  – initiating graceful shutdown"),
        _ = sigterm => tracing::info!("Received SIGTERM – initiating graceful shutdown"),
    }
}
