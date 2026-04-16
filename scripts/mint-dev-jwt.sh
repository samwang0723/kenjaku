#!/usr/bin/env bash
# Mint a dev JWT signed with the dev keypair.
# Requires: openssl, jq, base64.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEV_DIR="$ROOT_DIR/config/dev"

TENANT="public"
PRINCIPAL="dev-user"
TTL_HOURS=24
ISSUER="kenjaku-dev"
AUDIENCE="kenjaku-api"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tenant)    TENANT="$2"; shift 2 ;;
    --principal) PRINCIPAL="$2"; shift 2 ;;
    --ttl)
      # Parse duration like "24h" -> hours
      TTL_HOURS="${2%h}"; shift 2 ;;
    *) echo "Unknown arg: $1"; exit 1 ;;
  esac
done

PRIVATE_KEY="$DEV_DIR/private.pem"
if [[ ! -f "$PRIVATE_KEY" ]]; then
  echo "[mint-dev-jwt] No private key at $PRIVATE_KEY. Run generate-dev-keypair.sh first."
  exit 1
fi

NOW=$(date +%s)
EXP=$((NOW + TTL_HOURS * 3600))

# Build JWT header + payload
HEADER=$(printf '{"alg":"RS256","typ":"JWT"}' | openssl base64 -e -A | tr '+/' '-_' | tr -d '=')
PAYLOAD=$(printf '{"tenant_id":"%s","principal_id":"%s","plan_tier":"enterprise","iss":"%s","aud":"%s","iat":%d,"exp":%d}' \
  "$TENANT" "$PRINCIPAL" "$ISSUER" "$AUDIENCE" "$NOW" "$EXP" | openssl base64 -e -A | tr '+/' '-_' | tr -d '=')

# Sign
SIGNATURE=$(printf '%s.%s' "$HEADER" "$PAYLOAD" | \
  openssl dgst -sha256 -sign "$PRIVATE_KEY" -binary | \
  openssl base64 -e -A | tr '+/' '-_' | tr -d '=')

TOKEN="${HEADER}.${PAYLOAD}.${SIGNATURE}"

# Write to file
TOKEN_FILE="$DEV_DIR/dev-token.txt"
echo -n "$TOKEN" > "$TOKEN_FILE"

echo "[mint-dev-jwt] Token written to $TOKEN_FILE"
echo "[mint-dev-jwt] tenant=$TENANT principal=$PRINCIPAL ttl=${TTL_HOURS}h"
