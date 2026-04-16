#!/usr/bin/env bash
# Generate an RSA-2048 dev keypair for local JWT auth.
# Idempotent: skips if files exist, unless --force is passed.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEV_DIR="$ROOT_DIR/config/dev"

FORCE=false
for arg in "$@"; do
  case "$arg" in
    --force) FORCE=true ;;
  esac
done

mkdir -p "$DEV_DIR"

if [[ -f "$DEV_DIR/private.pem" && -f "$DEV_DIR/public.pem" && "$FORCE" == "false" ]]; then
  echo "[generate-dev-keypair] Keypair already exists at $DEV_DIR. Pass --force to regenerate."
  exit 0
fi

echo "[generate-dev-keypair] Generating RSA-2048 keypair..."
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$DEV_DIR/private.pem" 2>/dev/null
chmod 600 "$DEV_DIR/private.pem"
openssl pkey -in "$DEV_DIR/private.pem" -pubout -out "$DEV_DIR/public.pem" 2>/dev/null

echo "[generate-dev-keypair] Keypair written to $DEV_DIR/{private,public}.pem"
