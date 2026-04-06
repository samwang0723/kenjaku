.PHONY: build test run fmt lint clean \
       docker-build docker-up docker-down docker-logs docker-test \
       migrate migrate-revert ingest-url ingest-folder openapi check

# ── Local Development ───────────────────────────────────────────
build:
	cargo build

build-release:
	cargo build --release

check:
	cargo check --workspace

test:
	cargo test --workspace

test-verbose:
	cargo test --workspace -- --nocapture

run:
	APP_ENV=local cargo run --bin kenjaku-server

run-release:
	APP_ENV=local cargo run --release --bin kenjaku-server

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets -- -D warnings

clean:
	cargo clean

# ── Docker ──────────────────────────────────────────────────────
docker-build:
	docker compose build

docker-up: docker-build
	docker compose up -d

docker-down:
	docker compose down

docker-logs:
	docker compose logs -f

docker-restart:
	docker compose down && docker compose up -d --build

docker-ps:
	docker compose ps

# Run tests inside docker (infra must be up)
docker-test:
	docker compose up -d qdrant postgres redis
	@echo "Waiting for infra to be healthy..."
	@sleep 5
	APP_ENV=docker cargo test --workspace
	docker compose down

# ── Database ────────────────────────────────────────────────────
migrate:
	sqlx migrate run --source migrations

migrate-revert:
	sqlx migrate revert --source migrations

# ── Ingestion ───────────────────────────────────────────────────
ingest-url:
	@if [ -z "$(URL)" ]; then echo "Usage: make ingest-url URL=https://..."; exit 1; fi
	cargo run --bin kenjaku-ingest -- url --entry "$(URL)" --depth $(or $(DEPTH),2)

ingest-folder:
	@if [ -z "$(FOLDER)" ]; then echo "Usage: make ingest-folder FOLDER=./docs"; exit 1; fi
	cargo run --bin kenjaku-ingest -- folder --path "$(FOLDER)"

# ── OpenAPI ─────────────────────────────────────────────────────
openapi:
	@echo "Swagger UI available at: http://localhost:8080/swagger-ui when server is running"
