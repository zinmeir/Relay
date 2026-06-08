//! Configuration for the LLM Gateway.
//!
//! All values are read from environment variables at startup.  API keys are
//! **not** logged; the custom [`Debug`] implementation redacts them so they
//! cannot leak through tracing spans or error messages.

use std::{env, fmt};
use anyhow::{Context, Result};

// ─────────────────────────────────────────────────────────────────────────────

/// Gateway-wide configuration, read once from the environment at startup.
///
/// # Required environment variables
/// | Variable            | Description                      |
/// |---------------------|----------------------------------|
/// | `OPENAI_API_KEY`    | OpenAI secret key (`sk-…`)       |
/// | `ANTHROPIC_API_KEY` | Anthropic secret key (`sk-ant-…`)|
///
/// # Optional environment variables (defaults shown)
/// | Variable                | Default |
/// |-------------------------|---------|
/// | `GATEWAY_PORT`          | `8000`  |
/// | `PROVIDER_TIMEOUT_SECS` | `30`    |
/// | `CACHE_MAX_CAPACITY`    | `10000` |
/// | `CACHE_TTL_SECS`        | `3600`  |
pub struct Config {
    /// OpenAI API key (`OPENAI_API_KEY`).
    pub openai_api_key: String,
    /// Anthropic API key (`ANTHROPIC_API_KEY`).
    pub anthropic_api_key: String,
    /// TCP port the gateway's HTTP server listens on.
    pub port: u16,
    /// Per-request HTTP timeout applied to upstream provider calls (seconds).
    pub provider_timeout_secs: u64,
    /// Maximum number of entries retained in the in-memory response cache.
    pub cache_max_capacity: u64,
    /// Cache entry time-to-live; stale entries are evicted after this many seconds.
    pub cache_ttl_secs: u64,
}

impl Config {
    /// Load [`Config`] from environment variables.
    ///
    /// # Errors
    /// Returns an error if either required API key is absent, or if any
    /// optional variable is present but cannot be parsed into the expected type.
    pub fn from_env() -> Result<Self> {
        let openai_api_key = env::var("OPENAI_API_KEY")
            .context("OPENAI_API_KEY is not set")?;

        let anthropic_api_key = env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY is not set")?;

        let port                 = env_parse_or("GATEWAY_PORT",          8_000u16)?;
        let provider_timeout_secs = env_parse_or("PROVIDER_TIMEOUT_SECS", 30u64)?;
        let cache_max_capacity   = env_parse_or("CACHE_MAX_CAPACITY",    10_000u64)?;
        let cache_ttl_secs       = env_parse_or("CACHE_TTL_SECS",        3_600u64)?;

        Ok(Self {
            openai_api_key,
            anthropic_api_key,
            port,
            provider_timeout_secs,
            cache_max_capacity,
            cache_ttl_secs,
        })
    }
}

// ── Custom Debug – redacts secrets so they cannot leak into logs ──────────────

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("openai_api_key",       &"[REDACTED]")
            .field("anthropic_api_key",    &"[REDACTED]")
            .field("port",                 &self.port)
            .field("provider_timeout_secs",&self.provider_timeout_secs)
            .field("cache_max_capacity",   &self.cache_max_capacity)
            .field("cache_ttl_secs",       &self.cache_ttl_secs)
            .finish()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Read an optional environment variable and parse it into `T`.
///
/// Returns `default` if the variable is not set.
/// Returns an error if the variable is set but cannot be parsed.
fn env_parse_or<T>(key: &str, default: T) -> Result<T>
where
    T: std::str::FromStr + Copy,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match env::var(key) {
        Ok(raw) => raw
            .parse::<T>()
            .with_context(|| format!("Invalid value for environment variable `{key}`: {raw:?}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(env::VarError::NotUnicode(raw)) => {
            anyhow::bail!("Environment variable `{key}` contains non-UTF-8 data: {raw:?}")
        }
    }
}
