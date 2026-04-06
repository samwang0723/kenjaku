# Kenjaku

A production-grade Contextual RAG search engine built in Rust. Combines hybrid vector + BM25 retrieval with LLM-powered answer generation, intent classification, multi-locale translation, and a pluggable component layout system.

## Architecture

```
kenjaku-core       Domain types, traits, config, errors
kenjaku-infra      Qdrant, PostgreSQL, Redis, OpenAI, Gemini, Claude, OpenTelemetry
kenjaku-service    Search pipeline, hybrid retrieval, RRF reranking, trending, conversations
kenjaku-api        Axum HTTP handlers, rate limiting, input validation, SSE streaming
kenjaku-server     Binary with DI, graceful shutdown, background workers
kenjaku-ingest     CLI for document crawling, parsing, and chunking
```

See [docs/architect.md](docs/architect.md) for C4 diagrams, ADRs, and design details.

## Quick Start

### Prerequisites

- Rust 1.85+
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

The server will be available at `http://localhost:8080`.

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
curl -X POST http://localhost:8080/api/v1/search \
  -H "Content-Type: application/json" \
  -d '{
    "query": "How do I reset my password?",
    "locale": "en",
    "session_id": "sess-001",
    "request_id": "req-001",
    "streaming": false,
    "top_k": 10
  }'
```

### Supported Locales

`en`, `zh`, `zh-TW`, `ja`, `ko`, `de`, `fr`, `es`

## Ingestion

Ingest documents from a URL or local folder:

```bash
# Crawl a website
make ingest-url URL=https://docs.example.com

# Ingest local files (md, txt, html)
make ingest-folder FOLDER=./my-docs
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
| `make run` | Run server locally (APP_ENV=local) |
| `make docker-build` | Build Docker image |
| `make docker-up` | Build + start full stack |
| `make docker-down` | Stop all containers |
| `make docker-test` | Spin up infra, run tests, tear down |
| `make migrate` | Run database migrations |
| `make ingest-url URL=...` | Crawl and ingest from URL |
| `make ingest-folder FOLDER=...` | Ingest from local directory |

## Tech Stack

| Component | Technology |
|-----------|------------|
| Language | Rust 1.85 (edition 2024) |
| HTTP | Axum 0.8 + Tower middleware |
| Vector DB | Qdrant (cosine similarity + text index) |
| Database | PostgreSQL 17 (sqlx) |
| Cache | Redis 7 (trending sorted sets) |
| Embeddings | OpenAI text-embedding-3-small |
| LLM | Google Gemini (with google_search grounding) |
| Contextualizer | Anthropic Claude Haiku 4.5 |
| Observability | OpenTelemetry + structured JSON logging |
| Container | Alpine Linux (~30MB runtime image) |

## License

MIT
