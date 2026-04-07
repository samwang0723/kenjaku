# Kenjaku — Contextual RAG Search Engine Architecture

## 1. System Overview

Kenjaku is a contextual Retrieval-Augmented Generation (RAG) search engine built as a Rust workspace with 6 crates. It combines hybrid vector + full-text retrieval, LLM-based answer generation, intent classification, multi-locale translation, and a pluggable component layout system to deliver structured search responses.

### C4 Context Diagram

```mermaid
C4Context
    title System Context — Kenjaku RAG Search Engine

    Person(user, "End User", "Searches for information via API or UI")
    Person(admin, "Content Admin", "Ingests documents via CLI")

    System(kenjaku, "Kenjaku Search Engine", "Contextual RAG search with hybrid retrieval, LLM generation, and multi-locale support")

    System_Ext(openai, "OpenAI API", "text-embedding-3-small for vector embeddings")
    System_Ext(gemini, "Google Gemini API", "LLM generation, translation, suggestions, intent classification")
    System_Ext(claude, "Anthropic Claude API", "Chunk contextualization during ingestion")

    Rel(user, kenjaku, "Searches, provides feedback", "HTTPS/JSON + SSE")
    Rel(admin, kenjaku, "Ingests documents", "CLI")
    Rel(kenjaku, openai, "Generates embeddings", "HTTPS")
    Rel(kenjaku, gemini, "Generates answers, translates, classifies", "HTTPS")
    Rel(kenjaku, claude, "Contextualizes chunks", "HTTPS")
```

### C4 Container Diagram

```mermaid
C4Container
    title Container Diagram — Kenjaku

    Person(user, "End User")

    Container_Boundary(kenjaku, "Kenjaku System") {
        Container(geto, "geto-web", "nginx + vanilla JS", "Mobile phone-frame SPA. Same-origin reverse proxy to /api/, renders SSE start/delta/done events into a debug panel + streaming markdown answer")
        Container(server, "kenjaku-server", "Rust/Axum", "HTTP server, graceful shutdown, worker orchestration")
        Container(api, "kenjaku-api", "Rust/Axum", "REST endpoints, DTOs, middleware, SSE streaming")
        Container(service, "kenjaku-service", "Rust", "Search orchestration, hybrid retrieval, reranking, trending, feedback")
        Container(core, "kenjaku-core", "Rust", "Domain types, traits, config, errors")
        Container(infra, "kenjaku-infra", "Rust", "Provider implementations, DB clients, telemetry")
        Container(ingest, "kenjaku-ingest", "Rust/Clap", "CLI for document crawling, parsing, chunking")
    }

    ContainerDb(qdrant, "Qdrant", "Vector DB", "Embeddings + full-text index")
    ContainerDb(postgres, "PostgreSQL", "RDBMS", "Conversations, feedback, trending")
    ContainerDb(redis, "Redis", "Cache", "Real-time trending sorted sets")

    Rel(user, geto, "HTTPS (browser)")
    Rel(geto, api, "Reverse-proxy /api/, /health, /ready")
    Rel(api, service, "Rust calls")
    Rel(service, core, "Domain types")
    Rel(service, infra, "Data access")
    Rel(infra, core, "Implements traits")
    Rel(infra, qdrant, "gRPC")
    Rel(infra, postgres, "TCP/SQL")
    Rel(infra, redis, "RESP")
    Rel(server, api, "Builds router")
    Rel(server, service, "Creates services")
    Rel(server, infra, "Creates providers")
```

## 2. Crate Dependency Graph

```mermaid
graph TD
    SERVER[kenjaku-server<br/><i>binary</i>] --> API[kenjaku-api]
    SERVER --> SERVICE[kenjaku-service]
    SERVER --> INFRA[kenjaku-infra]
    SERVER --> CORE[kenjaku-core]

    API --> SERVICE
    API --> INFRA
    API --> CORE

    SERVICE --> INFRA
    SERVICE --> CORE

    INFRA --> CORE

    INGEST[kenjaku-ingest<br/><i>binary</i>] --> SERVICE
    INGEST --> INFRA
    INGEST --> CORE

    style CORE fill:#e1f5fe
    style INFRA fill:#fff3e0
    style SERVICE fill:#e8f5e9
    style API fill:#f3e5f5
    style SERVER fill:#fce4ec
    style INGEST fill:#fff9c4
```

| Crate | Depends On | Depended By |
|-------|-----------|-------------|
| `kenjaku-core` | (none — leaf) | infra, service, api, server, ingest |
| `kenjaku-infra` | core | service, api, server, ingest |
| `kenjaku-service` | core, infra | api, server, ingest |
| `kenjaku-api` | core, infra, service | server |
| `kenjaku-server` | core, infra, service, api | (binary) |
| `kenjaku-ingest` | core, infra, service | (binary) |

## 3. Design Principles & Golden Rules

### 3.1 Trait-Based Abstraction at the Core

All external dependencies are abstracted behind traits defined in `kenjaku-core::traits`:

| Trait | Purpose | Current Impl |
|-------|---------|-------------|
| `EmbeddingProvider` | Vector embedding generation | `OpenAiEmbeddingProvider` |
| `LlmProvider` | Answer generation, streaming, translation, suggestions | `GeminiProvider` |
| `Contextualizer` | Chunk contextualization during ingestion | `ClaudeContextualizer` |
| `IntentClassifier` | Query intent classification | `LlmIntentClassifier` |
| `Retriever` | Document retrieval | `HybridRetriever` |
| `Reranker` | Result fusion/reranking | `RrfReranker` |

**Golden Rule: Business logic in `service` never imports concrete provider types. It depends only on `Arc<dyn Trait>`.**

### 3.2 Immutable Domain Types

All domain types in `kenjaku-core::types` are immutable `#[derive(Clone, Serialize, Deserialize)]` structs. State transitions produce new values, not mutations.

### 3.3 Fire-and-Forget for Non-Critical Paths

Trending recording and conversation persistence are decoupled from the search hot path:
- **Trending**: Direct Redis ZINCRBY, errors logged but not propagated
- **Conversations**: Sent through a bounded `mpsc` channel, batch-flushed by a background worker

### 3.4 Configuration Hierarchy

```
config/base.yaml              — defaults, no secrets (committed)
config/{APP_ENV}.yaml          — env overrides (committed)
config/secrets.{env}.yaml      — API keys, DB credentials (gitignored)
KENJAKU__* env vars            — final override layer
```

Secrets MUST live in `secrets.{env}.yaml` or env vars. The server validates all required secrets at startup via `AppConfig::validate_secrets()` and fails fast with a clear message listing what's missing.

### 3.5 Supported Locales

Typed `Locale` enum enforced at the API boundary:

| Code | Language |
|------|----------|
| `en` | English |
| `zh` | Chinese (Simplified) |
| `zh-TW` | Chinese (Traditional) |
| `ja` | Japanese |
| `ko` | Korean |
| `de` | German |
| `fr` | French |
| `es` | Spanish |

Non-English queries are translated to English before retrieval.

### 3.6 Intent Classification

Every query is classified by intent before search:

| Intent | Description |
|--------|-------------|
| `factual` | Seeking specific factual information |
| `navigational` | Looking for a specific page or resource |
| `how_to` | Procedural or step-by-step question |
| `comparison` | Comparing options |
| `troubleshooting` | Diagnosing or fixing a problem |
| `exploratory` | Open-ended research |
| `conversational` | Chitchat (not a real search) |
| `unknown` | Cannot determine |

## 4. Search Pipeline Flow

```mermaid
sequenceDiagram
    participant C as Client
    participant H as Handler
    participant S as SearchService
    participant IC as IntentClassifier
    participant TS as TranslationService
    participant R as HybridRetriever
    participant E as EmbeddingProvider
    participant Q as Qdrant
    participant RR as RrfReranker
    participant L as LlmProvider
    participant CS as ComponentService
    participant T as TrendingService
    participant CV as ConversationService

    C->>H: POST /api/v1/search
    H->>S: search(SearchRequest)

    S->>IC: classify(query)
    IC->>L: generate(intent_prompt, [])
    L-->>IC: "factual"
    IC-->>S: Intent::Factual

    alt locale != en
        S->>TS: translate(query, locale)
        TS->>L: translate(text, from, to)
        L-->>TS: english_query
        TS-->>S: english_query
    end

    S->>R: retrieve(query, collection, top_k)

    par Vector Search
        R->>E: embed([query])
        E-->>R: [embedding]
        R->>Q: vector_search(embedding, k*10)
        Q-->>R: vector_results
    and Full-Text Search
        R->>Q: fulltext_search(query, k*10)
        Q-->>R: fulltext_results
    end

    R->>RR: rerank(vector, fulltext, top_k, weights)
    RR-->>R: merged_chunks
    R-->>S: chunks

    S->>L: generate(query, chunks)
    L-->>S: LlmResponse

    S->>L: suggest(query, answer)
    L-->>S: suggestions[]

    S->>CS: assemble(response, suggestions)
    CS-->>S: components[]

    par Fire-and-Forget
        S->>T: record_query(locale, query)
    and Async Persistence
        S->>CV: record(CreateConversation)
    end

    S-->>H: SearchResponse
    H-->>C: JSON response
```

## 5. Component System Design

The component system provides a pluggable, configurable layout for search responses:

```mermaid
graph LR
    subgraph "ComponentLayout (from config)"
        O[order: llm_answer, sources, suggestions]
    end

    subgraph "ComponentService.assemble()"
        A[LlmAnswerComponent]
        B[SourcesComponent]
        C[SuggestionsComponent]
    end

    O --> A --> B --> C

    subgraph "SearchResponse"
        R["components: Vec<Component>"]
    end

    C --> R
```

Adding a new component type requires:
1. New variant in `ComponentType` and `Component` enums
2. New component struct
3. New arm in `ComponentService::assemble()`
4. Update config YAML

## 6. Data Flow

### 6.1 Ingestion Pipeline

```mermaid
flowchart LR
    subgraph Input
        URL[URL Crawl]
        DIR[Local Folder]
    end

    subgraph Processing
        CLEAN["Strip Noise Tags<br/>script/style/nav/..."]
        MD["HTML -> Markdown<br/>(html2md)"]
        CHUNK["Token Chunking<br/>(tiktoken cl100k)<br/>800 tok / 100 overlap"]
        CTX["Contextualize<br/>via Claude Haiku<br/>(prompt caching)"]
        EMBED["Embed<br/>via OpenAI<br/>text-embedding-3-small"]
    end

    subgraph Storage
        QD[(Qdrant)]
    end

    URL --> CLEAN
    DIR --> CLEAN
    CLEAN --> MD
    MD --> CHUNK
    CHUNK --> CTX
    CTX --> EMBED
    EMBED --> QD
```

The URL crawler includes SSRF protection: a private-IP blocklist (RFC1918, loopback, link-local, CG-NAT), DNS resolution check before every fetch, and `redirect::Policy::none()` to prevent redirect-based bypass.

### 6.2 Query Pipeline

```mermaid
flowchart TD
    Q[User Query + Locale] --> IC{Intent Classify}
    Q --> TR{Needs Translation?}

    TR -->|Yes| TRANSLATE[LLM Translate to EN]
    TR -->|No| SEARCH_Q[Use Original]
    TRANSLATE --> SEARCH_Q

    SEARCH_Q --> VS["Vector Search<br/>Qdrant"]
    SEARCH_Q --> FT["Full-Text Search<br/>Qdrant"]

    VS --> RRF["RRF Reranker<br/>weighted merge"]
    FT --> RRF

    RRF --> LLM["LLM Generate<br/>with context"]
    LLM --> SUG["LLM Suggest<br/>follow-ups"]

    LLM --> ASM["Assemble<br/>Components"]
    SUG --> ASM

    ASM --> RESP[SearchResponse]

    Q --> TREND["Record Trending<br/>Redis ZINCRBY"]
    RESP --> CONV["Queue Conversation<br/>mpsc channel"]
```

## 7. Infrastructure Topology

```mermaid
graph TB
    subgraph "Application Layer"
        SRV["kenjaku-server<br/>:8080"]
        ING["kenjaku-ingest<br/>CLI"]
    end

    subgraph "Data Layer"
        QD[("Qdrant<br/>:6334<br/>Vectors + Text Index")]
        PG[("PostgreSQL<br/>:5432<br/>Conversations, Feedback, Trending")]
        RD[("Redis<br/>:6379<br/>Real-time Trending")]
    end

    subgraph "External APIs"
        OAI["OpenAI API<br/>Embeddings"]
        GEM["Gemini API<br/>LLM"]
        CLD["Claude API<br/>Contextualizer"]
    end

    subgraph "Observability"
        OTLP["OTLP Collector<br/>Traces"]
        LOG["JSON Structured Logs"]
    end

    SRV --> QD
    SRV --> PG
    SRV --> RD
    SRV --> OAI
    SRV --> GEM
    SRV --> OTLP
    SRV --> LOG

    ING --> QD
    ING --> OAI
    ING --> CLD
```

### Background Workers

| Worker | Trigger | Function |
|--------|---------|----------|
| `TrendingFlushWorker` | Timer (300s default) | Scans Redis `trending:*` keys, flushes entries above threshold to PostgreSQL |
| `ConversationFlushWorker` | Channel drain | Batch-inserts queued conversation records to PostgreSQL |

## 8. Database Schema

### PostgreSQL

```sql
-- Reason categories for negative feedback
CREATE TABLE reason_categories (
    id SERIAL PRIMARY KEY,
    slug VARCHAR(100) UNIQUE NOT NULL,
    label VARCHAR(255) NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT TRUE
);

-- User feedback on search responses
CREATE TABLE feedback (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id VARCHAR(255) NOT NULL,
    request_id VARCHAR(255) NOT NULL,
    action VARCHAR(20) NOT NULL CHECK (action IN ('like', 'dislike', 'cancel')),
    reason_category_id INTEGER REFERENCES reason_categories(id),
    description TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Popular/trending search queries (flushed from Redis)
CREATE TABLE popular_queries (
    id SERIAL PRIMARY KEY,
    locale VARCHAR(10) NOT NULL,
    query TEXT NOT NULL,
    search_count BIGINT NOT NULL DEFAULT 0,
    period VARCHAR(20) NOT NULL,
    period_date DATE NOT NULL,
    UNIQUE(locale, query, period, period_date)
);

-- Conversation records for analytics and audit
CREATE TABLE conversations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id VARCHAR(255) NOT NULL,
    request_id VARCHAR(255) NOT NULL UNIQUE,
    query TEXT NOT NULL,
    response_text TEXT NOT NULL,
    locale VARCHAR(10) NOT NULL,
    intent VARCHAR(50) NOT NULL,
    meta JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

### Qdrant Collection

```json
{
  "collection_name": "documents",
  "vectors": { "size": 1536, "distance": "Cosine" },
  "payload_indices": {
    "contextualized_content": "text (tokenized, lowercase, word)",
    "title": "text (tokenized, lowercase, word)"
  }
}
```

### Redis Key Patterns

```
trending:daily:{locale}:{YYYY-MM-DD}   -> ZSET (query -> score)  TTL: 2 days
trending:weekly:{locale}:{YYYY-W##}    -> ZSET (query -> score)  TTL: 14 days
```

## 9. Security Boundaries

```mermaid
graph TB
    subgraph "Trust Boundary: Internet"
        CLIENT[Client]
    end

    subgraph "Trust Boundary: API Gateway"
        AUTH["Auth + Rate Limit"]
    end

    subgraph "Trust Boundary: Application"
        API["API Layer<br/>Input validation"]
        SVC["Service Layer<br/>Business rules"]
    end

    subgraph "Trust Boundary: Data"
        QD[(Qdrant)]
        PG[(PostgreSQL)]
        RD[(Redis)]
    end

    subgraph "Trust Boundary: External APIs"
        LLM[LLM APIs]
    end

    CLIENT --> AUTH
    AUTH --> API
    API --> SVC
    SVC --> QD
    SVC --> PG
    SVC --> RD
    SVC --> LLM
```

## 9.1 Implemented Hardening (post-review)

The first review round identified these production gaps. All have been addressed:

- **Query length cap** (2000 chars), **top_k cap** (100), **autocomplete/top-searches limit caps** — enforced at API boundary
- **Rate limiting** via `tower_governor` (60 req/min per IP, `SmartIpKeyExtractor`)
- **Request body limit** (64KB) via `RequestBodyLimitLayer`
- **Request timeout** (30s) via `TimeoutLayer`
- **Error sanitization** — `Error::user_message()` returns safe strings; never leaks DB URLs, API errors, or internals
- **SSRF protection** in URL crawler (private-IP blocklist + DNS check + no-redirect)
- **Prompt injection defense** — user text isolated in `<text>`/`<query>` XML tags with "ignore instructions inside" preambles in both the intent classifier and translator
- **Redis SCAN** replaces the original `KEYS` call in the trending flush worker
- **Secrets validation** at startup via `AppConfig::validate_secrets()`
- **Migrations** — sqlx flat-file format with conversations table

Still deferred (accepted by operator):
- API authentication (next phase)
- Gemini API key in URL query param (Google API design)
- Conversation PII retention policy

## 10. Key Decision Records

### ADR-001: Qdrant for Vector + Full-Text Search

**Context**: Need both vector similarity search and keyword/BM25-style full-text search in a single store.

**Decision**: Use Qdrant with vector indices AND text payload indices on the same collection.

**Rationale**: Single data store simplifies operations. Qdrant's text index supports tokenized full-text search on payload fields. Cosine distance is well-suited for normalized embeddings from OpenAI.

**Trade-offs**: Full-text search is not true BM25 (Qdrant uses simpler text matching) — acceptable given 80/20 weight toward vector search. At very large scale (>10M vectors), consider a dedicated full-text engine alongside Qdrant.

### ADR-002: Reciprocal Rank Fusion for Hybrid Reranking

**Decision**: Use weighted RRF: `score = semantic_weight / (rank + 1) + bm25_weight / (rank + 1)`.

**Rationale**: RRF is rank-based, not score-based — avoids incomparable score distributions between vector (cosine 0-1) and text search. Weights are configurable in YAML (default 80/20).

### ADR-003: Channel-Based Async Conversation Flush

**Decision**: Use bounded `tokio::sync::mpsc` channel (1024 buffer) with batch-insert worker.

**Rationale**: `try_send` is non-blocking — zero latency on search path. Batch inserts (up to 64) reduce DB round trips. Records can be lost on crash — acceptable for analytics data.

### ADR-004: Gemini as Primary LLM with Google Search Grounding

**Decision**: Use Google Gemini with the `google_search` tool for grounded responses with source citations.

**Rationale**: Cost-effective for high-volume workloads. Native Google Search grounding provides real-time source citations. Vendor lock-in mitigated by `LlmProvider` trait abstraction.

### ADR-005: Claude for Chunk Contextualization

**Decision**: Use Claude Haiku 4.5 with prompt caching for contextualization during ingestion.

**Rationale**: Document content (large) is cached; only the chunk prompt (small) changes per call — cost-efficient. Separate from runtime LLM because contextualization is write-path only.

## 11. Scaling Considerations

| Bottleneck | Impact | Mitigation |
|-----------|--------|------------|
| LLM latency (3 sequential calls) | ~600ms minimum per request | Parallelize intent + translation; cache intent for repeated queries |
| Embedding latency | 50-200ms per query | Cache embeddings for repeated queries |
| Redis KEYS command | O(N) blocks Redis at scale | Replace with SCAN cursor-based iteration |
| PostgreSQL connections | Default pool of 10 | Increase + use PgBouncer in transaction mode |
| Single-process | Limited to one server | Deploy N replicas — all state in external stores |

**Projected capacity**: ~500-1000 QPS (limited by LLM latency). Intent classification and query normalization already run in parallel via `tokio::join!` in `SearchService::search`. With query-result caching layered on top, projected ~2000 QPS.

## 12. Measured Latency (post-optimization)

Test query: *"What rewards do I get with the Crypto.com prepaid card?"* against the ingested help.crypto.com corpus (64 chunks, depth=1).

| Metric | Non-streaming | Streaming (SSE) |
|--------|---------------|-----------------|
| TTFT | n/a | ~3.7s |
| Total time | ~6.8s | ~5.4s |
| Observed user experience | single JSON dump | tokens flow live |

Pipeline breakdown (streaming):
1. `max(intent_classify, translate)` in parallel — ~1.5s (translator is the bottleneck since it's slightly slower than intent classify)
2. Hybrid retrieve (vector + BM25 parallel via `try_join!`) — ~0.5s
3. Open Gemini `streamGenerateContent` connection — ~1.7s to first byte
4. Stream token deltas — ~1.7s for a typical answer

The intent classifier was previously the bottleneck at ~5s because every call sent the `google_search` grounding tool. Fix: `GeminiProvider::generate()` detects an empty context slice and skips the tool entirely, dropping intent classify to ~1s.

## 13. SSE Streaming Protocol

The `/api/v1/search` handler emits **named** SSE events so the geto-web client can populate its debug panel as soon as the preamble work is done — without waiting for the first LLM token.

`SearchService::search_stream` returns a `SearchStreamOutput` containing:

| Field | Type | Purpose |
|-------|------|---------|
| `start_metadata` | `StreamStartMetadata` | Everything known before the LLM begins (intent, translated_query, locale, retrieval_count, preamble_latency_ms, request_id, session_id) |
| `stream` | `Pin<Box<dyn Stream<Item = Result<StreamChunk>>>>` | Token deltas from the LLM |
| `context` | `StreamContext` | Bookkeeping (sources, instants, ids) consumed by `complete_stream()` after the stream finishes |

The handler then emits these events into a `mpsc::channel(100)` which is wrapped in `Sse::new(ReceiverStream::new(rx))`:

| Event | Payload | Sent when |
|-------|---------|-----------|
| `event: start` | `StreamStartMetadata` JSON | Once, before the first token |
| `event: delta` | `{"text": "..."}` | Per LLM token chunk |
| `event: done` | `StreamDoneMetadata` (`latency_ms`, `sources`, `suggestions`, `llm_model`) | After the last delta — built by `SearchService::complete_stream(context, accumulated_answer)` which also runs `LlmProvider::suggest()` and queues the conversation record |
| `event: error` | `{"error": "..."}` | On any failure (logged AND sent so the client sees it) |

The server-side parser uses the `eventsource-stream` crate to consume Gemini's `streamGenerateContent?alt=sse` response — do NOT hand-roll a parser, Gemini's separators vary across responses and the manual parser was buggy.

## 14. CI

`.github/workflows/ci.yml` runs on push to `main` and on PRs:

- **Rust stable** job — `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --locked`, `cargo test --locked`. Cache key `kenjaku-stable` via `Swatinem/rust-cache@v2`. Workspace-wide `RUSTFLAGS=-D warnings`.
- **Docker build** job (depends on Rust) — validates `docker compose config -q`, then builds both `kenjaku` and `geto-web` images via `docker/build-push-action@v6` with GHA cache.

Both jobs must pass before merge.
