//! In-memory response cache layer.
//!
//! This module is responsible for one thing: deriving a deterministic,
//! collision-resistant cache key from the raw bytes of an incoming request body.
//!
//! The actual cache store (`moka::future::Cache`) lives in [`crate::state::AppState`]
//! and is interacted with directly by handlers.
//!
//! ## Key strategy
//! Keys are **hex-encoded BLAKE3 hashes** of the raw request bytes.  BLAKE3 is
//! chosen because it is:
//! - Cryptographically sound (second-preimage resistant – no cache poisoning).
//! - Roughly 10× faster than SHA-256 on modern hardware.
//! - Produces a fixed-length 64-character hex string, ideal as a HashMap key.
//!
//! Two requests with *identical* JSON payloads (byte-for-byte) produce the
//! same cache key.  Semantically-equivalent but differently-serialised payloads
//! (e.g. different field order) will **not** match — this is intentional for
//! the MVP and avoids the cost of canonical serialisation.

/// Derive a 64-character hex-encoded BLAKE3 hash from raw request body bytes.
///
/// This value is used as the cache lookup key.
///
/// # Example
/// ```ignore
/// let key = cache::derive_key(b"{\"model\":\"gpt-4\",\"messages\":[]}");
/// assert_eq!(key.len(), 64);
/// ```
pub fn derive_key(body: &[u8]) -> String {
    blake3::hash(body).to_hex().to_string()
}
