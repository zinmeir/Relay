# Relay: An LLM Gateway

> A blazing-fast, open-core reverse proxy for AI providers — built in Rust.

Sits between your application and OpenAI / Anthropic. Drop-in compatible with
the OpenAI API. Provides **exact-match response caching** and **automatic model
fallback** with zero changes to your existing client code.

```
your app  ──POST /v1/chat/completions──►  LLM Gateway  ──►  OpenAI
                                                         └──► Anthropic (auto-fallback)
```

---

## Table of Contents

- [Features](#features)
- [System Requirements](#system-requirements)
- [Installation](#installation)
- [Configuration](#configuration)
- [Running the Server](#running-the-server)
- [API Reference](#api-reference)
- [Client Examples](#client-examples)
- [How Caching Works](#how-caching-works)
- [How Fallback Works](#how-fallback-works)
- [Observability & Logging](#observability--logging)
- [Architecture](#architecture)
- [Development](#development)
- [License](#license)

---

## Features

| Feature | Status |
|---|---|
| OpenAI-compatible `/v1/chat/completions` endpoint | ✅ |
| Exact-match response cache (BLAKE3 + moka) | ✅ |
| Automatic fallback: OpenAI → Anthropic on 5xx / timeout | ✅ |
| Structured tracing (`tracing` + `RUST_LOG`) | ✅ |
| Per-request UUID via `x-request-id` header | ✅ |
| Liveness probe (`GET /health`) | ✅ |
| Graceful shutdown on SIGINT / SIGTERM | ✅ |
| Streaming / SSE (`"stream": true`) | 🔜 |
| Semantic / vector routing | 🔜 |
| Rate limiting | 🔜 |
| Multi-tenant API key management | 🔜 |

---

## System Requirements

| Requirement | Minimum |
|---|---|
| OS | Linux, macOS, or Windows |
| Rust | 1.75 or later |
| RAM | ~50 MB at idle (scales with cache size) |
| OpenAI API key | Required |
| Anthropic API key | Required (for fallback) |

---

## Installation

### From zip (this release)

```bash
unzip llm-gateway.zip
cd llm-gateway
```

### From source

```bash
git clone https://github.com/your-org/llm-gateway
cd llm-gateway
```

### Install Rust (if needed)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustup update stable
```

---

## Configuration

All configuration is via environment variables. Copy the example file and edit it:

```bash
cp .env.example .env
```

### Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `OPENAI_API_KEY` | ✅ | — | Your OpenAI secret key (`sk-…`) |
| `ANTHROPIC_API_KEY` | ✅ | — | Your Anthropic secret key (`sk-ant-…`) |
| `GATEWAY_PORT` | No | `8000` | TCP port the server listens on |
| `PROVIDER_TIMEOUT_SECS` | No | `30` | Per-request timeout for upstream API calls (seconds) |
| `CACHE_MAX_CAPACITY` | No | `10000` | Maximum number of responses held in memory |
| `CACHE_TTL_SECS` | No | `3600` | How long a cached response is kept (seconds) |
| `RUST_LOG` | No | `llm_gateway=debug` | Log filter (see [Observability](#observability--logging)) |

### Example `.env`

```env
OPENAI_API_KEY=sk-...
ANTHROPIC_API_KEY=sk-ant-...

GATEWAY_PORT=8000
PROVIDER_TIMEOUT_SECS=30
CACHE_MAX_CAPACITY=10000
CACHE_TTL_SECS=3600

RUST_LOG=llm_gateway=info,tower_http=info
```

---

## Running the Server

### Development

```bash
cargo run
```

### Production (optimised binary)

```bash
cargo build --release
./target/release/llm-gateway
```

On first build, Rust compiles all dependencies — this takes 1–2 minutes.
Subsequent builds are incremental and much faster.

### Expected startup output

```
INFO llm_gateway: LLM Gateway starting up version="0.1.0"
INFO llm_gateway: Configuration loaded
INFO llm_gateway: In-memory response cache ready max_capacity=10000 ttl_secs=3600
INFO llm_gateway: HTTP server ready – awaiting connections local_addr=0.0.0.0:8000
```

Stop the server with `Ctrl+C`. It will drain in-flight requests before exiting.

---

## API Reference

### `POST /v1/chat/completions`

Accepts a standard OpenAI chat-completions request body and returns a standard
OpenAI chat-completion response — either from cache or from a provider.

**Request headers**

| Header | Value |
|---|---|
| `Content-Type` | `application/json` |

**Request body** — standard OpenAI format

```json
{
  "model": "gpt-4o",
  "messages": [
    { "role": "system", "content": "You are a helpful assistant." },
    { "role": "user",   "content": "What is the capital of France?" }
  ],
  "temperature": 0.7,
  "max_tokens": 256
}
```

> ⚠️ `"stream": true` is not yet supported and will return a `400` error.

**Response body** — standard OpenAI format

```json
{
  "id": "chatcmpl-abc123",
  "object": "chat.completion",
  "created": 1719000000,
  "model": "gpt-4o",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "The capital of France is Paris."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 25,
    "completion_tokens": 9,
    "total_tokens": 34
  }
}
```

**Response headers**

| Header | Description |
|---|---|
| `x-request-id` | UUID identifying this request (useful for log correlation) |

**Status codes**

| Code | Meaning |
|---|---|
| `200` | Success (from cache or provider) |
| `400` | Bad request — invalid JSON, missing `messages`, or streaming requested |
| `502` | Both providers failed |
| `504` | Provider request timed out |

**Error response body**

```json
{
  "error": {
    "message": "Streaming is not supported. Remove \"stream\": true from your request body.",
    "type": "Bad Request"
  }
}
```

---

### `GET /health`

Liveness probe for container orchestrators (Kubernetes, Docker).

```bash
curl http://localhost:8000/health
# → 200 OK
```

---

## Client Examples

### curl

```bash
curl http://localhost:8000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

### Python (openai SDK)

Just change the `base_url` — no other code changes needed:

```python
from openai import OpenAI

client = OpenAI(
    api_key="any-string",       # the gateway handles real auth
    base_url="http://localhost:8000/v1",
)

response = client.chat.completions.create(
    model="gpt-4o",
    messages=[{"role": "user", "content": "Hello!"}],
)
print(response.choices[0].message.content)
```

### Node.js (openai SDK)

```javascript
import OpenAI from "openai";

const client = new OpenAI({
  apiKey: "any-string",
  baseURL: "http://localhost:8000/v1",
});

const response = await client.chat.completions.create({
  model: "gpt-4o",
  messages: [{ role: "user", content: "Hello!" }],
});
console.log(response.choices[0].message.content);
```

---

## How Caching Works

The cache is **exact-match** — based on the raw bytes of the request body, not
semantic meaning.

```
Request body bytes
       │
       ▼
  BLAKE3 hash  →  64-char hex key
       │
       ▼
  moka cache lookup
       │
  ┌────┴────┐
  │   HIT   │  → return cached response instantly (no upstream call, no cost)
  └─────────┘
  ┌────┴────┐
  │  MISS   │  → call provider → store response → return response
  └─────────┘
```

**What counts as a cache hit:** The request body must be **byte-for-byte
identical**. Different field ordering, extra whitespace, or any change in value
(including model name, temperature, or message content) produces a different key
and a cache miss.

**Tuning the cache:**

| Goal | Setting |
|---|---|
| Cache more responses | Increase `CACHE_MAX_CAPACITY` |
| Keep responses longer | Increase `CACHE_TTL_SECS` |
| Always hit the provider live | Set `CACHE_TTL_SECS=0` |
| Reduce memory usage | Decrease `CACHE_MAX_CAPACITY` |

---

## How Fallback Works

```
1. Forward request to OpenAI
        │
        ├─ 2xx ──────────────────────────────────► return response ✅
        │
        ├─ 5xx or timeout ──► rewrite for Anthropic (claude-3-5-sonnet-20241022)
        │                              │
        │                              ├─ 2xx ──► return response ✅
        │                              │
        │                              └─ any error ──► 502 Bad Gateway ❌
        │
        └─ 4xx ──────────────────────────────────► return error to client ❌
                                                   (no fallback — client mistake)
```

**When fallback triggers:** OpenAI `5xx` responses or network timeouts.

**When fallback does NOT trigger:** OpenAI `4xx` responses (bad API key,
invalid request, content policy, etc.) — these indicate a client error that
Anthropic would reject for the same reason.

**Schema translation:** When the fallback fires, the gateway automatically
rewrites the request into Anthropic's Messages API format:
- `system` role messages are lifted to Anthropic's top-level `system` field
- Model is overridden to `claude-3-5-sonnet-20241022`
- `max_tokens` is defaulted to `1024` if not present (required by Anthropic)
- The Anthropic response is normalised back into OpenAI format before returning

The caller cannot tell which provider served the response.

---

## Observability & Logging

Logging is controlled by the `RUST_LOG` environment variable using
[tracing-subscriber's EnvFilter](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html).

### Log levels

| `RUST_LOG` value | Output |
|---|---|
| `llm_gateway=debug` | Everything: cache keys, provider details, request bodies |
| `llm_gateway=info` | Cache hits/misses, provider success/failure, startup |
| `llm_gateway=warn` | Provider fallback events, client errors |
| `llm_gateway=error` | Server-side failures only |

### Sample log output (info level)

```
INFO http_request{method=POST uri=/v1/chat/completions request_id=f3a1...}: Cache MISS – forwarding to provider
INFO http_request{method=POST uri=/v1/chat/completions request_id=f3a1...}: Attempting primary provider provider="openai"
INFO http_request{method=POST uri=/v1/chat/completions request_id=f3a1...}: Primary provider succeeded provider="openai"

INFO http_request{method=POST uri=/v1/chat/completions request_id=9b2c...}: Cache HIT – returning cached response without upstream call
```

### Request tracing

Every request gets a `x-request-id` UUID. It appears in:
- The response header (`x-request-id: <uuid>`)
- Every log line for that request (`request_id=<uuid>`)

This makes it easy to correlate a specific request across log lines.

---

## Architecture

```
src/
├── main.rs               Entry point: runtime, tracing, state, server bind,
│                         graceful shutdown on SIGINT/SIGTERM
├── config.rs             Typed config from env vars. Custom Debug redacts
│                         API keys so they never appear in logs.
├── state.rs              AppState struct (config + HTTP client + moka cache).
│                         Wrapped in Arc — cloning into handlers is O(1).
├── error.rs              GatewayError enum (thiserror). Each variant maps to
│                         an HTTP status code and JSON error body.
├── cache/
│   └── mod.rs            derive_key(&[u8]) → BLAKE3 hex string.
├── providers/
│   ├── mod.rs            complete_with_fallback() — orchestrates the
│   │                     OpenAI → Anthropic fallback policy.
│   ├── openai.rs         Forwards request to OpenAI verbatim.
│   └── anthropic.rs      Translates OpenAI → Anthropic request format,
│                         calls the Messages API, translates response back.
└── routes/
    ├── mod.rs            Router factory. Registers routes and applies
    │                     TraceLayer + SetRequestIdLayer middleware.
    └── chat.rs           POST /v1/chat/completions handler. Owns the
                          cache-check → provider-call → cache-store lifecycle.
```

### Key design decisions

**Zero bare `.unwrap()` calls** — every error is handled explicitly through
`Result`, `Option::unwrap_or`, or `anyhow::Context`. The codebase will not
panic on malformed input or provider errors.

**Single shared HTTP client** — `reqwest::Client` manages a connection pool
internally. One instance is shared across all request handlers via `Arc`,
avoiding redundant TCP/TLS handshakes.

**Arc-wrapped cache values** — cache values are `Arc<serde_json::Value>`.
A cache hit costs one atomic reference-count increment — no heap allocation,
no JSON clone.

**BLAKE3 for cache keys** — ~10× faster than SHA-256, cryptographically sound,
produces a fixed 64-char hex key.

---

## Development

### Run with verbose logging

```bash
RUST_LOG=llm_gateway=debug,tower_http=debug cargo run
```

### Check for errors without running

```bash
cargo check
```

### Run clippy (linter)

```bash
cargo clippy -- -D warnings
```

### Format code

```bash
cargo fmt
```

### Build optimised binary

```bash
cargo build --release
# binary → ./target/release/llm-gateway
```

---

## License

MIT — see [LICENSE](LICENSE) for details.
