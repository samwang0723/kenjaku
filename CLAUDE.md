# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
make build              # cargo build (debug)
make test               # cargo test --workspace
make test-verbose       # cargo test --workspace -- --nocapture
make lint               # cargo clippy --workspace --all-targets -- -D warnings
make fmt                # cargo fmt --all
make run                # APP_ENV=local cargo run --bin kenjaku-server
make docker-up          # build image + start full stack (qdrant, postgres, redis, otel, kenjaku)
make docker-test        # spin up infra, run tests, tear down
```

Run a single test: `cargo test -p kenjaku-core test_locale_from_str_valid`
Run tests for one crate: `cargo test -p kenjaku-service`

## Architecture

Rust workspace (edition 2024, MSRV 1.88) with 6 crates in a strict DAG:

```
core ← infra ← service ← api ← server
                    ↑              ↑
                  ingest ──────────┘
```

- **kenjaku-core** — Domain types (`Locale`, `Intent`, `Component`, `SearchRequest`, `Conversation`), traits (`EmbeddingProvider`, `LlmProvider`, `Contextualizer`, `IntentClassifier`, `Retriever`), config loader, error types. Zero external service dependencies.
- **kenjaku-infra** — Implements core traits: `OpenAiEmbeddingProvider`, `GeminiProvider`, `ClaudeContextualizer`, `QdrantClient`, `RedisClient`, PostgreSQL repos. All external I/O lives here.
- **kenjaku-service** — Business logic. `SearchService` orchestrates the RAG pipeline. `HybridRetriever` merges vector + BM25 via RRF. Background workers flush trending/conversations async.
- **kenjaku-api** — Axum handlers, DTOs, router with rate limiting (`tower_governor`), body limit, timeout. Converts between DTOs and domain types.
- **kenjaku-server** — Binary. Wires DI, spawns workers, graceful shutdown.
- **kenjaku-ingest** — CLI binary. Crawls URLs, strips HTML noise, converts to markdown via `html2md`, token-chunks via `tiktoken-rs` (cl100k_base), contextualizes via Claude, embeds via OpenAI, upserts to Qdrant.

**Key rule**: Service layer depends only on `Arc<dyn Trait>`, never on concrete infra types.

## Search Pipeline (SearchService::search)

1 & 2. **Intent classification + query normalization in parallel** via `tokio::join!`. The translator is also a normalizer: it auto-detects source language, fixes typos, canonicalizes domain terminology, and runs on every query (including English). Both failures degrade gracefully to raw query + `Unknown` intent.
3. Hybrid retrieval — vector and BM25 full-text searched in parallel via `tokio::try_join!` in `HybridRetriever`.
4. RRF reranking (weighted merge, 80/20 semantic/lexical by default).
5. LLM answer generation — Gemini with `google_search` grounding tool (only when context is non-empty).
6. Suggestion generation (LLM fallback to document titles).
7. Component assembly (configurable layout order from YAML).
8. Trending recording (fire-and-forget to Redis).
9. Conversation queuing (mpsc channel → background batch insert to PostgreSQL).

**Gemini provider optimization**: `LlmProvider::generate()` detects empty context (intent classifier, suggest, etc.) and skips the `google_search` tool + caps `max_tokens` to 256. This dropped intent classification from ~5s to ~1s.

## Error Handling

`kenjaku_core::error::Error` is the single error type. In API handlers, always use `e.user_message()` instead of `e.to_string()` — it returns safe messages that don't leak DB connection strings, API errors, or internal details. `Validation` and `NotFound` pass through their message; all infra errors return generic "service unavailable" strings.

## Config & Secrets

4-layer hierarchy: `config/base.yaml` → `config/{APP_ENV}.yaml` → `config/secrets.{APP_ENV}.yaml` → `KENJAKU__*` env vars. Secrets files are gitignored. `AppConfig::validate_secrets()` runs at startup and fails fast listing all missing secrets. Env var example: `KENJAKU__LLM__API_KEY=xxx`.

## Type Conventions

- `Locale` enum (en/zh/zh-TW/ja/ko/de/fr/es) — validated at API boundary via `FromStr`, used throughout as typed enum, serialized as BCP-47 tags. Note: the translator ignores `locale` and auto-detects the source language instead.
- `Intent` enum — classified per query, stored in metadata and conversations, serialized as snake_case.
- `Component` enum (tagged `#[serde(tag = "type")]`) — response layout order configured via `search.component_layout.order` in YAML.

## Ingest Pipeline

`kenjaku-ingest` is a full pipeline (not a scaffold). Commands: `make docker-ingest-url URL=...` or `make docker-ingest-folder FOLDER=...` to run inside the running container.

Chunking defaults: **800 tokens** per chunk, **100 token overlap**, cl100k_base tokenizer (matches OpenAI embeddings). Configurable via `--chunk-size` / `--chunk-overlap` CLI flags or `config/base.yaml`.

HTML extraction: strips `<script>/<style>/<nav>/<footer>/<header>/<aside>/<form>/<iframe>/<svg>/<noscript>/<template>` tags, selects `<main>` or `<article>` or `<body>`, converts to clean markdown via `html2md`, drops bare URLs and empty link/image artifacts.

SSRF protection: private IP blocklist (RFC1918, loopback, link-local, CG-NAT), DNS resolution check before every fetch, `redirect::Policy::none()`.

## Async Patterns

Two fire-and-forget pipelines decouple the search hot path from persistence:
- **Trending**: `TrendingService::record_query()` does Redis ZINCRBY, errors swallowed. `TrendingFlushWorker` periodically SCAN+flush to PostgreSQL.
- **Conversations**: `ConversationService::record()` sends via bounded `mpsc::channel(1024)` with `try_send` (never blocks). `ConversationFlushWorker` batch-inserts up to 64 records per flush.

## Input Validation Bounds

Query: max 2000 chars. `top_k`: max 100. Autocomplete limit: max 50. Top-searches limit: max 100. Rate limit: 60 req/min per IP via `tower_governor` with `SmartIpKeyExtractor` (requires `into_make_service_with_connect_info::<SocketAddr>()` on the listener to expose the peer address). Body limit: 64KB. Request timeout: 30s.

## SSE Streaming

The streaming path uses the `eventsource-stream` crate to parse Gemini's SSE response (do NOT hand-roll — Gemini's separators vary, and the manual parser was buggy). The handler spawns a tokio task that feeds events into a `mpsc::channel(100)`, and the response is `Sse::new(ReceiverStream::new(rx))`. Errors in the spawned task are logged AND sent as `event: error` SSE events so the client sees them.
