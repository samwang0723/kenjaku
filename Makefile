.PHONY: build test run fmt lint clean dev-setup \
       docker-build docker-up docker-down docker-logs docker-test \
       migrate migrate-revert ingest-url ingest-folder openapi check seed-admin

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

dev-setup:
	@scripts/generate-dev-keypair.sh
	@scripts/mint-dev-jwt.sh --tenant public --principal dev-user --ttl 24h
	@if [ -f config/dev/dev-token.txt ]; then cp config/dev/dev-token.txt geto-web/.dev-token; fi

run: dev-setup
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

# ── geto-web (visual frontend) ─────────────────────────────────
geto-web-build:
	docker compose build geto-web

geto-web-up: geto-web-build
	docker compose up -d geto-web
	@echo "geto-web available at http://localhost:3000"

geto-web-logs:
	docker compose logs -f geto-web

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

# auth-login-rbac: re-seed the default admin for a given tenant.
# The migration already seeds admin@public.com / admin for the public
# tenant at first-migrate-run time. Use this target after manually
# creating another tenant (e.g. via psql INSERT INTO tenants) to drop
# a default admin@{TENANT}.com / admin row. Idempotent via
# ON CONFLICT (email) DO NOTHING.
#
# Usage: make seed-admin TENANT=acme
# Requires: DATABASE_URL env var pointing at the live Postgres.
seed-admin:
	@if [ -z "$(TENANT)" ]; then echo "Usage: make seed-admin TENANT=acme"; exit 1; fi
	@if [ -z "$$DATABASE_URL" ]; then echo "Set DATABASE_URL=postgres://user:pass@host:port/db"; exit 1; fi
	@psql "$$DATABASE_URL" -v ON_ERROR_STOP=1 -c \
	  "INSERT INTO users (tenant_id, email, password_hash, role, enabled) \
	   VALUES ('$(TENANT)', 'admin@$(TENANT).com', \
	           '$$argon2id$$v=19$$m=19456,t=2,p=1$$kV5cbB/pe1yU0qbAQ+DrJg$$LYugS7g4BpDxojre7o8MtG9ul+oOxsdQgVkH17x9nEU', \
	           'admin', TRUE) \
	   ON CONFLICT (email) DO NOTHING;"
	@echo "Seeded admin@$(TENANT).com (password: admin)"

# ── Ingestion (local) ───────────────────────────────────────────
ingest-url:
	@if [ -z "$(URL)" ]; then echo "Usage: make ingest-url URL=https://..."; exit 1; fi
	APP_ENV=local cargo run --bin kenjaku-ingest -- url --entry "$(URL)" --depth $(or $(DEPTH),2)

ingest-folder:
	@if [ -z "$(FOLDER)" ]; then echo "Usage: make ingest-folder FOLDER=./docs"; exit 1; fi
	APP_ENV=local cargo run --bin kenjaku-ingest -- folder --path "$(FOLDER)"

# ── Ingestion (docker) ──────────────────────────────────────────
# Runs inside the kenjaku container so it uses the docker network and secrets.
docker-ingest-url:
	@if [ -z "$(URL)" ]; then echo "Usage: make docker-ingest-url URL=https://..."; exit 1; fi
	docker compose exec -e APP_ENV=docker kenjaku /app/kenjaku-ingest url --entry "$(URL)" --depth $(or $(DEPTH),2)

docker-ingest-folder:
	@if [ -z "$(FOLDER)" ]; then echo "Usage: make docker-ingest-folder FOLDER=./docs"; exit 1; fi
	docker compose exec -e APP_ENV=docker kenjaku /app/kenjaku-ingest folder --path "$(FOLDER)"

# ── OpenAPI ─────────────────────────────────────────────────────
openapi:
	@echo "Swagger UI available at: http://localhost:8080/swagger-ui when server is running"
