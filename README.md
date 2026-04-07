# Kenjaku

A production-grade Contextual RAG search engine built in Rust. Combines hybrid vector + BM25 retrieval with LLM-powered answer generation, intent classification, query normalization (typo fix + translation + term canonicalization), conversation storage, SSE streaming, and a pluggable component layout system.

## Architecture

```
kenjaku-core       Domain types, traits, config, errors
kenjaku-infra      Qdrant, PostgreSQL, Redis, OpenAI, Gemini, Claude, OpenTelemetry
kenjaku-service    Search pipeline, hybrid retrieval, RRF reranking, trending, conversations
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

# Start everything
make docker-up
```

- Backend API: `http://localhost:18080`
- Visual frontend (geto-web): `http://localhost:3000` — mobile phone-frame
  SPA that streams answers and renders source citations

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
| `GET` | `/api/v1/top-searches` | Popular queries by locale and period |
| `GET` | `/api/v1/autocomplete` | Query suggestions from trending + document titles |
| `POST` | `/api/v1/feedback` | User feedback (like/dislike/cancel) on responses |
| `GET` | `/health` | Liveness check |
| `GET` | `/ready` | Readiness check (qdrant + postgres + redis) |

### Search Example

```bash
# Non-streaming
curl -X POST http://localhost:18080/api/v1/search \
  -H "Content-Type: application/json" \
  -d '{
    "query": "How do I reset my password?",
    "locale": "en",
    "session_id": "sess-001",
    "request_id": "req-001",
    "streaming": false,
    "top_k": 10
  }'

# Streaming (SSE) — set streaming: true
curl -N -X POST http://localhost:18080/api/v1/search \
  -H "Content-Type: application/json" \
  -d '{"query":"How do I reset my password?","locale":"en","session_id":"s","request_id":"r","streaming":true,"top_k":5}'
```

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

## Configuration

Layered YAML config with secrets separation:

```
config/base.yaml              Defaults (committed)
config/{APP_ENV}.yaml          Environment overrides (committed)
config/secrets.{env}.yaml     API keys, DB credentials (gitignored)
KENJAKU__* env vars            Final override (e.g. KENJAKU__LLM__API_KEY)
```

`APP_ENV` values: `local` (default), `docker`, `staging`, `production`

## Makefile Targets

| Target | Description |
|--------|-------------|
| `make build` | Cargo build (debug) |
| `make test` | Run all workspace tests |
| `make lint` | Clippy with warnings as errors |
| `make fmt` | Format all code |
| `make run` | Run server locally (APP_ENV=local) |
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
| Cache | Redis 7 (trending sorted sets) |
| Embeddings | OpenAI text-embedding-3-small |
| LLM | Google Gemini (with google_search grounding) |
| Contextualizer | Anthropic Claude Haiku 4.5 |
| Observability | OpenTelemetry + structured JSON logging |
| Container | Alpine Linux (~30MB runtime image) |

## CI

GitHub Actions runs on every push to `main` and every PR
(`.github/workflows/ci.yml`):

- **Rust stable** — `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo build --locked`, `cargo test --locked` (cached via
  `Swatinem/rust-cache`)
- **Docker build** — validates `docker compose config`, then builds
  both the `kenjaku` and `geto-web` images via Buildx with GHA cache

## License

MIT
