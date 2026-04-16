---
name: refresh-dev-token
description: Re-mint the local dev JWT after it expires. Runs scripts/mint-dev-jwt.sh with the existing keypair, refreshes the file that docker-compose mounts into geto-web, and shows you what to do in the browser.
---

# /refresh-dev-token

The dev JWT at `config/dev/dev-token.txt` expires after 24h by default. When it does, the browser's cached token in `localStorage.bearerToken` returns 401 on every API call.

This skill re-mints the token in place. **No code changes, no re-running `make dev-setup`** — just regenerates the JWT with the existing keypair.

## Arguments

Parse the input after `/refresh-dev-token`:

| Input | Action |
|-------|--------|
| _(empty)_ | Re-mint for tenant `public`, principal `dev-user`, TTL 24h |
| `--tenant <name>` | Mint for a specific tenant (must exist in the `tenants` table) |
| `--ttl <hours>h` | Override TTL (e.g., `--ttl 168h` for 7 days) |
| `--principal <name>` | Override principal claim |

## Steps

### 1. Verify keypair exists

```bash
test -f config/dev/private.pem && test -f config/dev/public.pem \
  || (echo "Keypair missing — run 'make dev-setup' first" && exit 1)
```

If the keypair is missing entirely (first-time setup), bail and tell the user to run `make dev-setup`.

### 2. Show current token status (before re-minting)

Decode the existing token's expiry so the user can sanity-check. Use a plain bash one-liner (no `jq` dependency since Copilot flagged that earlier):

```bash
if [ -f config/dev/dev-token.txt ]; then
  PAYLOAD=$(cut -d. -f2 config/dev/dev-token.txt | tr -- '-_' '+/' | base64 -d 2>/dev/null || true)
  EXP=$(echo "$PAYLOAD" | grep -o '"exp":[0-9]*' | cut -d: -f2)
  if [ -n "$EXP" ]; then
    NOW=$(date +%s)
    if [ "$EXP" -gt "$NOW" ]; then
      REMAINING=$(( (EXP - NOW) / 60 ))
      echo "Current token expires in $REMAINING minutes (exp=$EXP)"
    else
      AGO=$(( (NOW - EXP) / 60 ))
      echo "Current token EXPIRED $AGO minutes ago (exp=$EXP)"
    fi
  fi
fi
```

### 3. Re-mint

```bash
bash scripts/mint-dev-jwt.sh
```

With optional overrides:
```bash
bash scripts/mint-dev-jwt.sh --tenant <name> --ttl <hours>h --principal <name>
```

The script writes the new JWT to `config/dev/dev-token.txt`. Docker's bind-mount (`type: bind, source: ./config/dev/dev-token.txt`) means the geto-web nginx container sees the new file immediately — **no container restart needed**.

### 4. Verify the new token

```bash
# Decode the new exp
NEW_EXP=$(cut -d. -f2 config/dev/dev-token.txt | tr -- '-_' '+/' | base64 -d 2>/dev/null \
  | grep -o '"exp":[0-9]*' | cut -d: -f2)
HUMAN_EXP=$(date -r "$NEW_EXP" 2>/dev/null || date -d "@$NEW_EXP" 2>/dev/null)
echo "New token valid until: $HUMAN_EXP"

# Smoke test — does it work end-to-end?
DEV_JWT=$(cat config/dev/dev-token.txt)
curl -sf -X POST http://localhost:18080/api/v1/search \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $DEV_JWT" \
  -H 'X-Session-Id: refresh-check' \
  -d '{"query":"ping","top_k":1}' --max-time 10 \
  | head -c 120 && echo
```

Expected: `200` with `"success":true`. If `401`, the kenjaku-server may be running against a stale public key — `docker compose -p kenjaku restart kenjaku` to reload.

### 5. Browser action

The browser still has the old expired token in `localStorage.bearerToken`. Tell the user to do one of these (no automatic fix — see the refresh-dev-token design decision in the session notes):

```
The new token is live on the server. To pick it up in the browser:

1. Open http://localhost:3000 in Chrome
2. DevTools → Application → Local Storage → http://localhost:3000
3. Delete the `bearerToken` entry
4. Reload the page

Or use the debug panel: paste the fresh token directly into the "Bearer Token"
field. Copy it to clipboard:

  pbcopy < config/dev/dev-token.txt   # macOS
  xclip -sel clip < config/dev/dev-token.txt   # Linux
```

## Summary report

After all steps, print a compact summary:

```
Dev token refreshed.
  Tenant:      public (or override)
  Principal:   dev-user (or override)
  Valid until: 2026-04-17 15:30:00
  Smoke test:  PASS (200 OK on /api/v1/search)
  File:        config/dev/dev-token.txt
Browser: clear localStorage.bearerToken and reload, or paste the fresh token.
```

## Tips

- If you renew frequently, consider increasing the default TTL: `/refresh-dev-token --ttl 168h` for a 7-day token. Only do this locally — production tokens should stay short-lived.
- If the smoke test fails with 401, the most likely cause is the kenjaku container is using a stale copy of `public.pem` cached in memory at boot. Restart: `docker compose -p kenjaku restart kenjaku`.
- This skill does **not** touch the private key or regenerate the keypair. If the keypair itself needs rotation, run `make dev-setup` (which regenerates everything and then calls `mint-dev-jwt.sh`).
- Staging/production never reads `config/dev/`, so this skill is a no-op outside local dev.
