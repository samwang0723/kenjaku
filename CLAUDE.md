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

- **kenjaku-core** — Domain types (`Locale`, `Intent`, `Component`, `SearchRequest`, `Conversation`), traits (`EmbeddingProvider`, `LlmProvider`, `Contextualizer`, `IntentClassifier`, `Tool`, `Brain`), types (`Message`, `Role`, `ContentPart`, `ToolId`, `ToolRequest`, `ToolOutput`, `ToolOutputMap`, `ToolError`, `ToolConfig`, `ServiceTier`), config loader, error types. Zero external service dependencies. `LlmProvider::generate` accepts `&[Message]` (the LLM-agnostic message type); `generate_brief` has a default impl wrapping a single user message.
- **kenjaku-infra** — Implements core traits: `OpenAiEmbeddingProvider`, `GeminiProvider`, `ClaudeContextualizer`, `QdrantClient`, `RedisClient`, PostgreSQL repos, plus `TitleResolver` (resolves Gemini grounding redirect URLs into real page titles via HTML parsing + Redis cache). `GeminiProvider` implements `messages_to_wire(&[Message]) -> (Option<GeminiContent>, Vec<GeminiContent>)` to map the core `Message` type to Gemini's wire format. Includes `estimate_cost` for per-call cost estimation based on model + `ServiceTier` (standard/flex/priority), and sends `serviceTier` in requests. All external I/O lives here.
- **kenjaku-service** — Business logic organized into 5 layer folders with only `lib.rs` + `search.rs` at the top level. `SearchService` is a thin shim; the real orchestration lives in `SearchOrchestrator` (in `harness/mod.rs`). The 5 layers:
  - **brain/** — `Brain` trait impl, `ConversationAssembler` (parallel `Vec<Message>` builder), `translation.rs` (translator), `intent.rs` (classifier), `prompt.rs` (system instruction + user turn text builders, moved from `GeminiProvider`), `generator.rs` (wraps `LlmProvider` over `&[Message]`).
  - **tools/** — `Tool` trait implementations: `rag.rs` (DocRagTool wrapping `HybridRetriever`), `brave_web.rs` (BraveWebTool), `retriever.rs`, `reranker.rs`, `config.rs`, `mod.rs`.
  - **session/** — `conversation.rs`, `history.rs`, `locale_memory.rs`, `feedback.rs`, `autocomplete.rs`.
  - **foundation/** — `quality.rs` (gibberish guard, trending normalization), `trending.rs`, `suggestion.rs`, `worker/` (trending flush, suggestion refresh).
  - **harness/** — `mod.rs` (`SearchOrchestrator` — owns `ToolTunnel`, cancel, merge), `fanout.rs` (`ToolTunnel` DAG executor — topo sort via Kahn's algorithm, tiered `join_all`), `context.rs` (`ToolOutputMap` to `ToolContext` merger), `component.rs` (layout assembly).
- **kenjaku-api** — Axum handlers, DTOs, router with rate limiting (`tower_governor`), body limit, timeout. Converts between DTOs and domain types.
- **kenjaku-server** — Binary. Wires DI, spawns workers, graceful shutdown.
- **kenjaku-ingest** — CLI binary. Crawls URLs, strips HTML noise, converts to markdown via `html2md`, token-chunks via `tiktoken-rs` (cl100k_base), contextualizes via Claude, embeds via OpenAI, upserts to Qdrant.

Plus a sibling static-site frontend (not a Cargo crate):

- **geto-web/** — Vanilla JS phone-frame SPA served by nginx. Talks to the kenjaku API via a same-origin reverse proxy (no CORS). Renders the SSE `start`/`delta`/`done` events into a debug panel + streaming markdown answer with clickable `[Source N]` chips.

**Key rule**: Service layer depends only on `Arc<dyn Trait>`, never on concrete infra types.

## Search Pipeline (SearchOrchestrator::search)

The search pipeline is orchestrated by `SearchOrchestrator` in `harness/mod.rs` (`SearchService` is a thin shim that delegates to it):

1 & 2. **Brain classifies intent + translates query in parallel** via `tokio::join!`. The translator is also a normalizer: it auto-detects source language, fixes typos, canonicalizes domain terminology, and runs on every query (including English). Both failures degrade gracefully to raw query + `Unknown` intent.
3. **ToolTunnel fans out tools by dependency tier.** Tier 0 (no deps, parallel): `DocRagTool` (wraps `HybridRetriever` — vector + BM25 via RRF). Tier 1 (depends on DocRag): `BraveWebTool` — reads `prior.chunk_count("doc_rag")` to decide whether to fire. Tools declare dependencies via `depends_on() -> Vec<ToolId>`; the tunnel topologically sorts them into tiers at startup (Kahn's algorithm). Per-tool errors degrade to `ToolOutput::Empty`; `BadRequest` propagates as `Error::Validation`. A per-request `CancellationToken` cascades through every tool on SSE disconnect via `CancelGuard`.
4. **Brain generates answer via `&[Message]`** — the `ConversationAssembler` builds a `Vec<Message>` from history + query + retrieved context. `LlmProvider::generate` / `generate_stream` accepts `&[Message]`; `GeminiProvider` maps these to its wire format via `messages_to_wire`. Gemini with `google_search` grounding tool. The system prompt (in `brain/prompt.rs`) actively encourages google_search fallback when the internal corpus is incomplete instead of refusing. Streaming path captures grounding sources from `groundingMetadata.groundingChunks[].web` on each SSE event.
5. **Post-generation TitleResolver** — resolves each grounding URL in parallel (follows redirects, parses HTML for page titles, Redis-cached).
6. Suggestion generation (LLM fallback to document titles).
7. Component assembly (configurable layout order from YAML).
8. Trending recording (fire-and-forget to Redis) — wrapped in `quality::is_gibberish` reject + `quality::normalize_for_trending` (English: translator output, others: raw; both capitalized at first char).
9. Conversation queuing (mpsc channel → background batch insert to PostgreSQL).

**Gemini provider optimization**: `LlmProvider::generate_brief()` (default impl on the trait) wraps a single user message and calls `generate`. `GeminiProvider` overrides it to skip the `google_search` tool + cap `max_tokens` to 400 (raised from 256 to leave headroom for CJK→English translation outputs near the 500-char input cap). This dropped intent classification from ~5s to ~1s.

**Gemini provider features**: `messages_to_wire(&[Message])` converts the LLM-agnostic `Message` type to `Vec<GeminiContent>`, extracting the system instruction separately. `estimate_cost` calculates per-call cost based on model + `ServiceTier` (standard/flex/priority). The `serviceTier` field (lowercase) is sent in every Gemini API request. Every provider method that hits Gemini (`generate`, `generate_brief`, `generate_stream`, `translate`, `suggest`) now returns an `Option<LlmUsage>` alongside its result, populated from the response's `usageMetadata`. The streaming path attaches `LlmUsage` to the terminal `StreamChunk.usage` field. A free `cost_rates_for_model(model) -> (input_per_m, output_per_m)` helper keeps the pricing table shared between the sync and stream code paths.

**Gemini response deserialization**: All `GeminiResponse` structs use `#[serde(rename_all = "camelCase")]`. Gemini's REST API returns camelCase (`groundingMetadata`, `finishReason`, `usageMetadata`). Combined with `#[serde(default)]`, missing this annotation causes silent empty `Option`s instead of parse errors — this was the original cause of the streaming-path grounding capture being broken.

## Error Handling

`kenjaku_core::error::Error` is the single error type. In API handlers, always use `e.user_message()` instead of `e.to_string()` — it returns safe messages that don't leak DB connection strings, API errors, or internal details. `Validation` and `NotFound` pass through their message; all infra errors return generic "service unavailable" strings.

## Config & Secrets

4-layer hierarchy: `config/base.yaml` → `config/{APP_ENV}.yaml` → `config/secrets.{APP_ENV}.yaml` → `KENJAKU__*` env vars. Secrets files are gitignored. `AppConfig::validate_secrets()` runs at startup and fails fast listing all missing secrets. Env var example: `KENJAKU__LLM__API_KEY=xxx`.

Notable config keys:
- `llm.service_tier` — `ServiceTier` enum (standard/flex/priority). Controls Gemini API `serviceTier` field and cost estimation multiplier. Default: `standard`.
- `schedule_cron` — 6-field cron format (sec min hour day month weekday), NOT 5-field POSIX. Example: `0 0 3 * * *` = daily at 03:00 UTC.

## Type Conventions

- `Locale` enum (en/zh/zh-TW/ja/ko/de/fr/es) — used throughout as typed enum, serialized as BCP-47 tags. **`/search` no longer accepts `locale` in the request body** (PR #7) — the translator detects it from the query text and the LLM answer is pinned to that locale via Gemini `systemInstruction`. **`/top-searches` and `/autocomplete`** resolve locale via the `ResolvedLocale` Axum extractor: `?locale=` override → session memory (Redis, 2h TTL, keyed by `X-Session-Id` header or `?session_id=`) → `Accept-Language` header → `en` default. The `/search` handler writes the detected locale into session memory fire-and-forget so subsequent GETs from the same device inherit it.
- `Intent` enum — classified per query, stored in metadata and conversations, serialized as snake_case.
- `Component` enum (tagged `#[serde(tag = "type")]`) — response layout order configured via `search.component_layout.order` in YAML.
- `Tool` trait (`kenjaku-core/src/traits/tool.rs`) — pluggable external tool contract: `id()`, `config()`, `depends_on()`, `should_fire()`, `invoke()`. Implementations in `kenjaku-service/src/tools/`.
- `Brain` trait (`kenjaku-core/src/traits/brain.rs`) — LLM facade: `classify_intent`, `translate`, `generate`, `generate_stream`, `suggest`. Takes `&[Message]` + `&ToolContext`. All non-streaming methods return `Result<(T, Option<LlmCall>)>` so the pipeline can roll per-call token + cost accounting into `SearchMetadata.usage` / `StreamDoneMetadata.usage`. Streaming usage arrives on the terminal `StreamChunk.usage`.
- `UsageStats`, `LlmCall`, `SharedUsageTracker` (`kenjaku-core/src/types/usage.rs`) — per-request LLM accounting. `UsageStats { input_tokens, output_tokens, total_tokens, estimated_cost_usd, calls: Vec<LlmCall> }`. `LlmCall { purpose, model, input_tokens, output_tokens, cost_usd, latency_ms }`. `SharedUsageTracker` is an `Arc<Mutex<UsageStats>>` wrapper the pipeline passes through `StreamContext.usage` so concurrent `tokio::join!` calls can push entries without extra plumbing. Surfaced on `SearchMetadata.usage` (non-streaming) and `StreamDoneMetadata.usage` (streaming); `StreamStartMetadata` deliberately omits it since tokens aren't tallied until the LLM finishes.
- `Message`, `Role`, `ContentPart` (`kenjaku-core/src/types/message.rs`) — LLM-agnostic message type. `Role::System`/`User`/`Assistant`. `ContentPart::Text(String)` with future extension points for tool calls and images.
- `ToolId`, `ToolRequest`, `ToolOutput`, `ToolOutputMap`, `ToolError`, `ToolConfig` (`kenjaku-core/src/types/tool.rs`) — tool execution types. `ToolOutput` is a tagged enum: `Chunks`, `WebHits`, `Structured`, `Empty`. `ToolOutputMap` wraps `HashMap<ToolId, ToolOutput>` with typed accessors (`chunk_count`, `has_web_hits`, `get`) and a deterministic `insertion_order` for reproducible iteration.
- `ServiceTier` enum (`kenjaku-core/src/config.rs`) — standard/flex/priority. Controls Gemini API tier and cost multiplier.

## Ingest Pipeline

`kenjaku-ingest` is a full pipeline (not a scaffold). Commands: `make docker-ingest-url URL=...` or `make docker-ingest-folder FOLDER=...` to run inside the running container.

Chunking defaults: **800 tokens** per chunk, **100 token overlap**, cl100k_base tokenizer (matches OpenAI embeddings). Configurable via `--chunk-size` / `--chunk-overlap` CLI flags or `config/base.yaml`.

HTML extraction: strips `<script>/<style>/<nav>/<footer>/<header>/<aside>/<form>/<iframe>/<svg>/<noscript>/<template>` tags, selects `<main>` or `<article>` or `<body>`, converts to clean markdown via `html2md`, drops bare URLs and empty link/image artifacts.

SSRF protection: private IP blocklist (RFC1918, loopback, link-local, CG-NAT), DNS resolution check before every fetch, `redirect::Policy::none()`.

## Async Patterns

**Cancellation**: A per-request `CancellationToken` (from `tokio-util`) threads through every `Tool::invoke` call and the `ToolTunnel` fan-out. On SSE client disconnect, `CancelGuard` (defined in `search.rs`) drops and calls `token.cancel()`, cascading cancellation to all in-flight tool calls, the conversation assembler, and the title resolver. Tools that ignore the token are backstopped by `tokio::time::timeout(tool_budget_ms)`.

**ToolTunnel**: Replaces the old direct retriever/web-search calls. Tools declare dependencies via `depends_on() -> Vec<ToolId>`. The tunnel topologically sorts them into tiers at startup (Kahn's algorithm, cycle detection panics at boot). Within each tier, tools run in parallel via `join_all`. Between tiers, execution is serial so dependent tools can read prior outputs from `ToolOutputMap`.

Two fire-and-forget pipelines decouple the search hot path from persistence:
- **Trending**: `TrendingService::record_query(locale, raw, normalized)` first runs `quality::is_gibberish(raw)` (length caps, no-space Latin, single-char dominance) and drops obviously bad queries; then stores via `quality::normalize_for_trending` (English: translator-normalized, capitalized; other locales: raw, capitalized). Errors swallowed. `TrendingFlushWorker` periodically SCAN+flushes entries above `popularity_threshold` to PostgreSQL. The read paths (autocomplete + top-searches) additionally enforce `crowd_sourcing_min_count` (default 2) so anything that slips past the record-time guard still needs independent repeated searches before surfacing.
- **Conversations**: `ConversationService::record()` sends via bounded `mpsc::channel(1024)` with `try_send` (never blocks). `ConversationFlushWorker` batch-inserts up to 64 records per flush.
- **Feedback**: Now upserts on `(session_id, request_id)` (unique index added in migration `20260407000001_feedback_unique`) — repeated like/dislike clicks update the existing row in place instead of duplicating.
- **Locale memory**: `LocaleMemory::record(session_id, locale)` writes a sticky per-device locale to Redis (key prefix `sl:`, 2h TTL). Bounded at 128 chars on both write and read. Errors swallowed.
- **Default suggestions refresh**: `SuggestionRefreshWorker::run_scheduled` runs on a cron schedule (default `0 0 3 * * *`) and is also exposed via `kenjaku-ingest seed-refresh-now [--force] [--dry-run]`. Holds a PG advisory lock on a pinned `PoolConnection`, computes a SHA-256 corpus fingerprint over `(collection_name, points_count, sorted first 32 point ids)`, short-circuits if unchanged. Otherwise: scrolls Qdrant points-with-vectors → mini-batch k-means via `linfa-clustering` (deterministic seeded `StdRng`) → one multi-locale Gemini call per cluster (`responseMimeType=application/json` + 8-locale `responseSchema`, wrapped in `tokio::time::timeout`) → safety regex filter (price/forecast/buy-sell prompts dropped) → atomic swap via `refresh_batches.status` enum (`running`/`active`/`superseded`/`failed`, single-active partial unique index) → retain last N batches via FK `ON DELETE CASCADE` (excluding `running` rows). Steady-state: ~0 LLM calls/day; on corpus change: ~20 calls.

## Suggestion Blending

`SuggestionService::get_top` and `autocomplete` load active `default_suggestions` for the resolved locale plus crowdsourced `popular_queries`, then run **Efraimidis-Spirakis weighted random sampling without replacement**: each item's key is `-ln(U) / weight`, sort ascending, take first K. The injectable `ServiceRng` uses `from_entropy` in production and `from_seed` in tests. Returned `BlendedItemDto` carries `{query, source, score}` so the frontend debug panel can show provenance (`default` vs `crowdsourced`).

## Input Validation Bounds

Query: max 2000 chars. `top_k`: max 100. Autocomplete limit: max 50. Top-searches limit: max 100. Rate limit: 60 req/min per IP via `tower_governor` with `SmartIpKeyExtractor` (requires `into_make_service_with_connect_info::<SocketAddr>()` on the listener to expose the peer address). Body limit: 64KB. Request timeout: 30s.

## SSE Streaming

The streaming path uses the `eventsource-stream` crate to parse Gemini's SSE response (do NOT hand-roll — Gemini's separators vary, and the manual parser was buggy).

`SearchService::search_stream` (delegating to `SearchOrchestrator`) returns a `SearchStreamOutput` containing:

- `start_metadata: StreamStartMetadata` — everything known before the LLM begins producing tokens (intent, translated_query, locale, retrieval_count, preamble_latency_ms, request_id, session_id)
- `stream` — the LLM token stream (`Pin<Box<dyn Stream<Item = Result<StreamChunk>>>>`). Each `StreamChunk` may carry an optional `grounding: Vec<LlmSource>` populated from Gemini's `groundingMetadata`, typically only on the final event with `finishReason`.
- `context: StreamContext` — bookkeeping (sources, instants, ids, `CancelGuard`) consumed by `complete_stream()` when the token stream finishes. The `CancelGuard` cancels the `CancellationToken` on drop, ensuring SSE client disconnect cascades to all in-flight work.

The `/api/v1/search` handler emits **named** SSE events into a `mpsc::channel(100)`:

- `event: start` → `StreamStartMetadata` JSON, sent once before the first token
- `event: delta` → `{"text": "..."}` per token from the LLM
- `event: done` → `StreamDoneMetadata` (`latency_ms`, `sources`, `suggestions`, `llm_model`, `usage`). The handler accumulates grounding sources + the last-seen `usageMetadata` from each chunk's `grounding`/`usage` fields while draining the stream, then calls `SearchService::complete_stream(context, accumulated_answer, grounding_sources, generator_call)`. That method resolves each grounding URL in parallel via `TitleResolver` (follows redirects, parses `<head>` for `og:title`/`twitter:title`/`<title>`/JSON-LD `headline`, Redis-cached 24h on success / 10min on failure), then merges grounding sources first followed by internal chunk sources, deduped by URL — grounding wins on conflict because it carries the resolved page title.
- `event: error` → `{"error": "..."}` on any failure (logged AND sent so the client sees it)

Errors in the spawned task are logged AND sent as `event: error` SSE events so the client sees them.

## CI

`.github/workflows/ci.yml` runs on push to `main` and on PRs:

- **Rust stable** job — `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --locked`, `cargo test --locked`. Uses `Swatinem/rust-cache` keyed on `kenjaku-stable`.
- **Docker build** job (depends on Rust) — validates `docker compose config -q`, then builds both `kenjaku` and `geto-web` images via Buildx with GHA cache.

`CARGO_TERM_COLOR=always` and `RUSTFLAGS=-D warnings` are set workspace-wide. When clippy fires a new lint locally that the CI catches first, fix it the same way you would any compile error — don't add `#[allow]` unless the lint is genuinely wrong for the case (the existing `clippy::too_many_arguments` allows on the ingest pipeline functions are an example of an intentional escape hatch).
