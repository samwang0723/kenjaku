.PHONY: build test run fmt lint clean docker-up docker-down migrate ingest-url ingest-folder openapi check

# Build
build:
	cargo build

build-release:
	cargo build --release

check:
	cargo check --workspace

# Test
test:
	cargo test --workspace

test-verbose:
	cargo test --workspace -- --nocapture

# Run
run:
	APP_ENV=local cargo run --bin kenjaku-server

run-release:
	APP_ENV=local cargo run --release --bin kenjaku-server

# Format & Lint
fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets -- -D warnings

# Docker
docker-up:
	docker compose up -d

docker-down:
	docker compose down

docker-logs:
	docker compose logs -f

# Database
migrate:
	sqlx migrate run --source migrations

migrate-revert:
	sqlx migrate revert --source migrations

# Ingestion
ingest-url:
	@if [ -z "$(URL)" ]; then echo "Usage: make ingest-url URL=https://..."; exit 1; fi
	cargo run --bin kenjaku-ingest -- url --entry "$(URL)" --depth $(or $(DEPTH),2)

ingest-folder:
	@if [ -z "$(PATH)" ]; then echo "Usage: make ingest-folder PATH=./docs"; exit 1; fi
	cargo run --bin kenjaku-ingest -- folder --path "$(PATH)"

# OpenAPI
openapi:
	@echo "OpenAPI spec available at: openapi/openapi.yaml"
	@echo "Swagger UI available at: http://localhost:8080/swagger-ui when server is running"

# Clean
clean:
	cargo clean
