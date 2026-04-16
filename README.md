# Kenjaku

A production-grade Contextual RAG search engine built in Rust. Combines hybrid vector + BM25 retrieval with LLM-powered answer generation, intent classification, query normalization (typo fix + translation + term canonicalization), conversation storage, SSE streaming, and a pluggable component layout system.

## Architecture

```
kenjaku-core       Domain types (Message, ToolId, ToolOutput, ServiceTier, ...),
                   traits (LlmProvider, Tool, Brain, EmbeddingProvider, ...),
                   config, errors
kenjaku-infra      Qdrant, PostgreSQL, Redis, OpenAI, Gemini (messages_to_wire,
                   serviceTier, estimate_cost), Claude, TitleResolver, OpenTelemetry
kenjaku-service    5-layer architecture:
                     brain/       — Brain facade, translator, classifier, prompt
                                    builders, ConversationAssembler, generator
                     tools/       — Tool impls: DocRagTool, BraveWebTool
                     session/     — conversation, history, locale memory, feedback
                     foundation/  — quality guard, trending, suggestion, workers
                     harness/     — SearchOrchestrator, ToolTunnel DAG executor,
                                    context merger, CancellationToken cascade
kenjaku-api        Axum HTTP handlers, rate limiting, input validation, SSE streaming
kenjaku-server     Binary with DI, graceful shutdown, background workers
kenjaku-ingest     CLI for document crawling, parsing, and chunking

geto-web/          Static phone-frame SPA UI (vanilla JS + nginx) talking
                   to the kenjaku API via same-origin reverse proxy
```

See [docs/architect.md](docs/architect.md) for C4 diagrams, ADRs, and design details.

## Quick Start

### Prerequisites

- Rust 1.88+
- Docker & Docker Compose

### Setup

```bash
# Clone
git clone git@github.com:samwang0723/kenjaku.git && cd kenjaku

# Create secrets config
cp config/secrets.example.yaml config/secrets.docker.yaml
# Edit config/secrets.docker.yaml with your API keys:
#   - postgres.url
#   - embedding.api_key (OpenAI)
#   - llm.api_key (Google Gemini)
#   - contextualizer.api_key (Anthropic Claude)

# Provision dev JWT keypair (required — tenancy is always-on)
make dev-setup

# Start everything
make docker-up
```

`make dev-setup` generates an RSA-2048 keypair at `config/dev/` (gitignored) and mints a signed JWT for the `public` tenant at `config/dev/dev-token.txt`. The kenjaku container mounts the public key for validation; the geto-web container mounts the token read-only and serves it at `/.dev-token` (localhost-only, blocked in public deployments). Without this step, kenjaku-server refuses to start when it later loads/stats `tenancy.jwt.public_key_path` — the configured key file is missing or not a regular file.

- Backend API: `http://localhost:18080`
- Visual frontend (geto-web): `http://localhost:3000` — mobile phone-frame
  SPA that streams answers, renders source citations, and exposes an
  8-locale language switcher (sets explicit `?locale=` override + sticky
  `X-Session-Id`)

Internal ports (exposed only for local debugging):

| Service | Host port | Container port |
|---------|-----------|----------------|
| kenjaku | 18080 | 8080 |
| geto-web | 3000 | 80 |
| qdrant  | 6333 / 6334 | 6333 / 6334 |
| postgres | 15432 | 5432 |
| redis    | 16379 | 6379 |
| otel-collector | 4317 / 4318 | 4317 / 4318 |

Open the Qdrant dashboard at http://localhost:6333/dashboard to inspect the vector collection.

### Local Development

```bash
# Start infra only
docker compose up -d qdrant postgres redis

# Create local secrets
cp config/secrets.example.yaml config/secrets.local.yaml
# Edit with your API keys

# Run server
make run

# Run tests
make test

# Lint
make lint
```

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1/search` | RAG search with optional SSE streaming |
| `GET` | `/api/v1/top-searches` | Blended default suggestions + crowdsourced popular queries (locale resolved via `?locale=` → session memory → `Accept-Language` → `en`) |
| `GET` | `/api/v1/autocomplete` | Prefix-matched blend of default suggestions + trending (gated by `crowd_sourcing_min_count`) + prettified document titles |
| `POST` | `/api/v1/feedback` | User feedback (like/dislike/cancel) on responses |
| `GET` | `/health` | Liveness check |
| `GET` | `/ready` | Readiness check (qdrant + postgres + redis) |

### Search Example

All authenticated endpoints require a Bearer JWT (tenancy-first — no anonymous access). For local dev, read the minted token and attach it:

```bash
export DEV_JWT=$(cat config/dev/dev-token.txt)

# Non-streaming — locale is auto-detected by the translator from the query text
curl -X POST http://localhost:18080/api/v1/search \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $DEV_JWT" \
  -H "X-Session-Id: sess-001" \
  -d '{
    "query": "How do I reset my password?",
    "session_id": "sess-001",
    "request_id": "req-001",
    "streaming": false,
    "top_k": 10
  }'

# Streaming (SSE) — set streaming: true
curl -N -X POST http://localhost:18080/api/v1/search \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $DEV_JWT" \
  -H "X-Session-Id: s" \
  -d '{"query":"How do I reset my password?","session_id":"s","request_id":"r","streaming":true,"top_k":5}'
```

Requests without a valid Bearer JWT return `401 {"success":false,"error":"Unauthorized tenant"}`. The response metadata includes per-request LLM `usage` (token counts + estimated cost per call) for observability.

### Supported Locales

`en`, `zh`, `zh-TW`, `ja`, `ko`, `de`, `fr`, `es`

## Ingestion

Ingest documents from a URL or local folder. The pipeline crawls → strips noise tags → converts to markdown → chunks at token boundaries (800 tok, 100 overlap) → contextualizes each chunk via Claude (with prompt caching) → embeds via OpenAI → stores in Qdrant.

```bash
# Crawl a website (runs locally, requires config/secrets.local.yaml)
make ingest-url URL=https://docs.example.com

# Ingest local files (md, txt, html)
make ingest-folder FOLDER=./my-docs

# Run ingestion inside the running docker stack (uses secrets.docker.yaml)
make docker-ingest-url URL=https://docs.example.com
make docker-ingest-folder FOLDER=./my-docs
```

### Default Suggestions Refresh

A daily background worker (`SuggestionRefreshWorker`, default `03:00 UTC`)
clusters the Qdrant corpus via mini-batch k-means and asks Gemini to
generate ≥10 native-language seed questions per cluster across all 8
locales in a single multi-locale call. Results are atomically swapped
into `default_suggestions` via the `refresh_batches.status` enum and
blended at read time with crowdsourced trending via
Efraimidis-Spirakis weighted random sampling. A SHA-256 corpus
fingerprint short-circuits unchanged runs to zero LLM calls.

To force a refresh on demand (inside the running container):

```bash
docker compose exec kenjaku kenjaku-ingest seed-refresh-now --force
docker compose exec kenjaku kenjaku-ingest seed-refresh-now --dry-run
```

## Configuration

Layered YAML config with secrets separation:

```
config/base.yaml              Defaults (committed)
config/{APP_ENV}.yaml          Environment overrides (committed)
config/secrets.{env}.yaml     API keys, DB credentials (gitignored)
config/dev/                   Dev JWT keypair + minted token (gitignored, via make dev-setup)
KENJAKU__* env vars            Final override (e.g. KENJAKU__LLM__API_KEY, KENJAKU__TENANCY__JWT__ISSUER)
```

`APP_ENV` values: `local` (default), `docker`, `staging`, `production`

### Tenancy

Tenancy is **always-on** post-3e. Every request requires a valid Bearer JWT (RS256/RS384/RS512/ES256/ES384 — `none` and HS* rejected). Required `tenancy.jwt.*` fields: `public_key_path`, `issuer`, `audience`. Staging and production deployments must provision these before boot or `validate_secrets()` fails fast.

Tenants are seeded in the `tenants` table (public seeded by the 3a migration). Collections are routed per-tenant via `{base_name}_{tenant_id}` naming (the `public` alias bridges legacy data).

## Makefile Targets

| Target | Description |
|--------|-------------|
| `make build` | Cargo build (debug) |
| `make test` | Run all workspace tests |
| `make lint` | Clippy with warnings as errors |
| `make fmt` | Format all code |
| `make dev-setup` | Generate RSA-2048 keypair + mint dev JWT (required for tenancy-first auth) |
| `make run` | Run server locally (APP_ENV=local; prereqs `dev-setup`) |
| `make docker-build` | Build Docker image |
| `make docker-up` | Build + start full stack |
| `make docker-down` | Stop all containers |
| `make docker-restart` | Rebuild and restart |
| `make docker-logs` | Follow container logs |
| `make docker-ps` | Show container status |
| `make docker-test` | Spin up infra, run tests, tear down |
| `make migrate` | Run database migrations |
| `make ingest-url URL=...` | Crawl and ingest (local) |
| `make ingest-folder FOLDER=...` | Ingest local directory |
| `make docker-ingest-url URL=...` | Ingest via the running kenjaku container |
| `make docker-ingest-folder FOLDER=...` | Ingest folder via container |
| `make geto-web-build` | Build the geto-web frontend image |
| `make geto-web-up` | Build + start geto-web (depends on a healthy kenjaku) |
| `make geto-web-logs` | Follow geto-web container logs |

## Tech Stack

| Component | Technology |
|-----------|------------|
| Language | Rust 1.88 (edition 2024) |
| HTTP | Axum 0.8 + Tower middleware |
| Vector DB | Qdrant (cosine similarity + text index) |
| Database | PostgreSQL 17 (sqlx) |
| Cache | Redis 7 (trending sorted sets + resolved title cache) |
| Embeddings | OpenAI text-embedding-3-small |
| LLM | Google Gemini (with `google_search` grounding — sources merged into responses, real page titles resolved) |
| Contextualizer | Anthropic Claude Haiku 4.5 |
| Observability | OpenTelemetry + structured JSON logging |
| Container | Alpine Linux (~30MB runtime image) |

## CI

GitHub Actions runs on every push to `main` and every PR
(`.github/workflows/ci.yml`):

- **Rust stable** — `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo build --locked`, `cargo test --locked` (cached via
  `Swatinem/rust-cache`), plus a **semgrep** step that runs
  `.semgrep/tenant-scope.yml` against `crates/kenjaku-infra/src/postgres/`
  to block tenant-blind SQL regressions
- **Docker build** — validates `docker compose config`, then builds
  both the `kenjaku` and `geto-web` images via Buildx with GHA cache

### Local regression matrix

A project-local `/regression` skill (under `.claude/skills/regression/`)
bundles the full 6-phase verification: local Rust build/test, canonical
Docker build+test (`make docker-test`), semgrep guardrail + fixture
self-test, docker deploy + authenticated API e2e smoke, optional
chrome-cdp web UI verification, and fire-and-forget worker health log
scan. Useful before opening a PR.

## License

MIT
