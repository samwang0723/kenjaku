# ── Stage 1: Build ──────────────────────────────────────────────
FROM rust:1.85-alpine AS builder

RUN apk add --no-cache musl-dev pkgconfig

WORKDIR /app

# Cache dependencies — copy manifests first
COPY Cargo.toml Cargo.lock ./
COPY crates/kenjaku-core/Cargo.toml crates/kenjaku-core/Cargo.toml
COPY crates/kenjaku-infra/Cargo.toml crates/kenjaku-infra/Cargo.toml
COPY crates/kenjaku-service/Cargo.toml crates/kenjaku-service/Cargo.toml
COPY crates/kenjaku-api/Cargo.toml crates/kenjaku-api/Cargo.toml
COPY crates/kenjaku-server/Cargo.toml crates/kenjaku-server/Cargo.toml
COPY crates/kenjaku-ingest/Cargo.toml crates/kenjaku-ingest/Cargo.toml

# Create dummy src files so cargo can resolve the workspace
RUN mkdir -p crates/kenjaku-core/src && echo "pub fn _dummy() {}" > crates/kenjaku-core/src/lib.rs \
    && mkdir -p crates/kenjaku-infra/src && echo "pub fn _dummy() {}" > crates/kenjaku-infra/src/lib.rs \
    && mkdir -p crates/kenjaku-service/src && echo "pub fn _dummy() {}" > crates/kenjaku-service/src/lib.rs \
    && mkdir -p crates/kenjaku-api/src && echo "pub fn _dummy() {}" > crates/kenjaku-api/src/lib.rs \
    && mkdir -p crates/kenjaku-server/src && echo "fn main() {}" > crates/kenjaku-server/src/main.rs \
    && mkdir -p crates/kenjaku-ingest/src && echo "fn main() {}" > crates/kenjaku-ingest/src/main.rs

# Build dependencies only (cached until Cargo.toml/Cargo.lock change)
RUN cargo build --release --workspace 2>/dev/null || true

# Copy real source and rebuild
COPY crates/ crates/
COPY migrations/ migrations/
COPY config/base.yaml config/base.yaml
COPY config/docker.yaml config/docker.yaml

# Touch all source files to invalidate the dummy build
RUN find crates/ -name "*.rs" -exec touch {} +

RUN cargo build --release --bin kenjaku-server --bin kenjaku-ingest

# ── Stage 2: Runtime ────────────────────────────────────────────
FROM alpine:3.21 AS runtime

RUN apk add --no-cache ca-certificates curl

RUN addgroup -g 1000 kenjaku \
    && adduser -u 1000 -G kenjaku -s /bin/sh -D kenjaku

WORKDIR /app

# Copy binaries (statically linked with musl)
COPY --from=builder /app/target/release/kenjaku-server /app/kenjaku-server
COPY --from=builder /app/target/release/kenjaku-ingest /app/kenjaku-ingest

# Copy config and migrations (secrets injected at runtime via volume mount)
COPY config/base.yaml config/base.yaml
COPY config/docker.yaml config/docker.yaml
COPY migrations/ migrations/

USER kenjaku

EXPOSE 8080

HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=3 \
    CMD ["curl", "-sf", "http://localhost:8080/health"]

ENTRYPOINT ["/app/kenjaku-server"]
