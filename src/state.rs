//! Shared application state.
//!
//! [`AppState`] is constructed once at startup and distributed to every Axum
//! handler via [`axum::extract::State`].  The outer [`std::sync::Arc`] wrapper
//! makes cloning into handler tasks O(1); all inner fields are either
//! cheaply-cloneable reference-counted types or primitives.

use std::sync::Arc;

use moka::future::Cache;

use crate::config::Config;

// ─────────────────────────────────────────────────────────────────────────────
// Type aliases
// ─────────────────────────────────────────────────────────────────────────────

/// A heap-allocated, atomically reference-counted JSON response value.
///
/// Storing `Arc<serde_json::Value>` in the cache means that inserting a
/// response and handing a clone to the caller is O(1): no deep copies occur.
pub type CachedResponse = Arc<serde_json::Value>;

/// Convenience alias for the concrete moka async cache type used throughout
/// the gateway.  Keyed by a hex-encoded BLAKE3 hash of the request body.
pub type ResponseCache = Cache<String, CachedResponse>;

// ─────────────────────────────────────────────────────────────────────────────
// AppState
// ─────────────────────────────────────────────────────────────────────────────

/// Shared application state injected into every Axum handler via
/// `axum::extract::State<Arc<AppState>>`.
///
/// ## Thread safety
/// Every field is `Send + Sync`:
/// - [`Config`] contains only `String` / numeric primitives.
/// - [`reqwest::Client`] is internally `Arc`-backed.
/// - [`moka::future::Cache`] is designed for concurrent access.
pub struct AppState {
    /// Gateway configuration (API keys, timeouts, cache parameters).
    pub config: Config,

    /// Shared HTTP client with a managed connection pool.
    ///
    /// `reqwest::Client` is internally reference-counted; cloning it is O(1)
    /// and does **not** create a new connection pool.
    pub http_client: reqwest::Client,

    /// In-memory, size- and time-bounded response cache.
    ///
    /// `moka::future::Cache` is itself internally `Arc`-backed; cloning the
    /// handle is O(1) and all clones share the same underlying storage.
    pub cache: ResponseCache,
}

impl AppState {
    /// Construct a new [`AppState`] from its constituent parts.
    ///
    /// Call this exactly once at startup and wrap the result in [`Arc`]:
    ///
    /// ```rust,ignore
    /// let state = Arc::new(AppState::new(config, http_client, cache));
    /// ```
    pub fn new(config: Config, http_client: reqwest::Client, cache: ResponseCache) -> Self {
        Self { config, http_client, cache }
    }
}
