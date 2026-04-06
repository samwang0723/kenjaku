#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/ingest_folder.sh <PATH> [COLLECTION]
# Example: ./scripts/ingest_folder.sh ./docs documents

FOLDER_PATH="${1:?Usage: $0 <PATH> [COLLECTION]}"
COLLECTION="${2:-documents}"

if [ ! -d "${FOLDER_PATH}" ]; then
    echo "Error: ${FOLDER_PATH} is not a directory"
    exit 1
fi

echo "Ingesting from folder: ${FOLDER_PATH} (collection: ${COLLECTION})"

cargo run --bin kenjaku-ingest -- folder \
    --path "${FOLDER_PATH}" \
    --collection "${COLLECTION}"
