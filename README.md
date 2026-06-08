# LLM Gateway

> A blazing-fast, open-core reverse proxy for AI providers — built in Rust.

Sits between your application and OpenAI / Anthropic. Drop-in compatible with
the OpenAI API. Provides **exact-match response caching** and **automatic model
fallback** without any changes to your client code.

```
your app  ──POST /v1/chat/completions──►  LLM Gateway  ──►  OpenAI
                                                         └──► Anthropic (fallback)
```

## Features

| Feature | Status |
|---|---|
| OpenAI-compatible endpoint | ✅ |
| Exact-match response cache (BLAKE3 + moka) | ✅ |
| Automatic fallback: OpenAI → Anthropic | ✅ |
| Structured tracing (`tracing` + `RUST_LOG`) | ✅ |
| Per-request UUID (`x-request-id`) | ✅ |
| Liveness probe (`GET /health`) | ✅ |
| Graceful shutdown (SIGINT / SIGTERM) | ✅ |
| Streaming (SSE) | 🔜 |
| Semantic / vector routing | 🔜 |

## Quick Start

### Prerequisites
- Rust 1.75+ (`rustup update stable`)
- OpenAI API key
- Anthropic API key

### Run

```bash
git clone https://github.com/your-org/llm-gateway
cd llm-gateway
cp .env.example .env
# Edit .env with your API keys
cargo run --release
```

### Configure

Copy `.env.example` to `.env` and fill in your keys:

```env
OPENAI_API_KEY=sk-...
ANTHROPIC_API_KEY=sk-ant-...

# Optional
GATEWAY_PORT=8000
PROVIDER_TIMEOUT_SECS=30
CACHE_MAX_CAPACITY=10000
CACHE_TTL_SECS=3600
RUST_LOG=llm_gateway=info,tower_http=info
```

## Usage

Point any OpenAI-compatible client at `http://localhost:8000`:

```bash
curl http://localhost:8000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

> **Note:** Streaming (`"stream": true`) is not yet supported.

## Architecture

```
src/
├── main.rs            Runtime bootstrap, tracing, state wiring, graceful shutdown
├── config.rs          Typed env-var configuration (secrets redacted from Debug)
├── state.rs           AppState (config + HTTP client + moka cache)
├── error.rs           GatewayError enum → HTTP JSON responses
├── cache/mod.rs       BLAKE3 cache key derivation
├── providers/
│   ├── mod.rs         Fallback orchestration (OpenAI → Anthropic)
│   ├── openai.rs      OpenAI provider (pass-through)
│   └── anthropic.rs   Anthropic provider (bidirectional schema translation)
└── routes/
    ├── mod.rs         Router factory + middleware stack
    └── chat.rs        POST /v1/chat/completions handler
```

## Caching

Responses are cached in-process using [moka](https://github.com/moka-rs/moka)
(a Caffeine-inspired concurrent cache). The cache key is the **BLAKE3 hash of
the raw request body bytes**. Two requests with identical JSON (byte-for-byte)
return the cached response without hitting any upstream provider.

Tune cache size and TTL via `CACHE_MAX_CAPACITY` and `CACHE_TTL_SECS`.

## Provider Fallback

1. Every request is forwarded to **OpenAI** first.
2. If OpenAI returns a `5xx` status or times out, the request is **transparently
   rewritten** for the **Anthropic Messages API** (`claude-3-5-sonnet-20241022`)
   and retried.
3. If both providers fail, the caller receives a `502 Bad Gateway`.
4. `4xx` errors from OpenAI are **not** retried (they indicate a client mistake
   that Anthropic would reject too).

## License

Apache 2.0
