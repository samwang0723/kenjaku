#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/ingest_url.sh <URL> [DEPTH]
# Example: ./scripts/ingest_url.sh https://docs.example.com 3

URL="${1:?Usage: $0 <URL> [DEPTH]}"
DEPTH="${2:-2}"
COLLECTION="${3:-documents}"

echo "Ingesting from URL: ${URL} (depth: ${DEPTH}, collection: ${COLLECTION})"

cargo run --bin kenjaku-ingest -- url \
    --entry "${URL}" \
    --depth "${DEPTH}" \
    --collection "${COLLECTION}"
