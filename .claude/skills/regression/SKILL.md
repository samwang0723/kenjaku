---
name: regression
description: Full regression test suite — local + Docker build/test, semgrep guardrails, API e2e, chrome-cdp web UI verification, and worker health log scan
---

# /regression — Full Regression Test Suite

Run the complete verification matrix for kenjaku. Build/test runs in **both** environments: local `cargo` for fast iteration + editor feedback, Docker for canonical truth (matches CI/prod exactly). If they disagree, Docker wins — but catching divergences early is itself useful signal.

> **Note on `cargo` commands below:** If you use `rtk` (Rust Token Killer) locally, pipe them through it for reduced token output (`rtk cargo ...`). Plain `cargo` is functionally equivalent and is the assumed default for any environment that does not ship `rtk`.

Covers:
1. Local Rust build + lint + tests (fast — uses incremental host cache)
2. Docker build + test (canonical — matches CI/prod)
3. Semgrep tenant-scope guardrail (runs on host against source files)
4. Docker deploy + API e2e smoke (rebuild running container, verify endpoints)
5. Chrome-cdp web UI verification (if Chrome debugger available)
6. Fire-and-forget worker health (trending flush, conversation flush, suggestion refresh)

## Arguments

| Input | Action |
|-------|--------|
| _(empty)_ | Run all 6 phases |
| `quick` | Phases 1+3 only (local build/test + semgrep — fastest pre-commit check) |
| `api` | Phases 1-4 (local + docker + semgrep + deploy + API smoke, no chrome) |
| `full` | All 6 phases (same as empty) |

## Phase 1: Local Rust Build + Lint + Tests

Fast host-side feedback loop. Uses the incremental cargo cache so iteration is seconds, not minutes. Runs on the developer's toolchain — may diverge from CI in edge cases, which is why Phase 2 also runs Docker.

```bash
# Build
cargo build --workspace

# Lint (clippy treats warnings as errors)
cargo clippy --workspace --all-targets -- -D warnings

# Format check
cargo fmt --all -- --check

# Unit + integration tests
cargo test --workspace
```

**Pass criteria:**
- Build: 0 errors
- Clippy: 0 warnings
- Fmt: 0 diffs
- Tests: 0 failures (report total pass count)

**On failure:** Stop and report. Do NOT proceed to later phases.

## Phase 2: Docker Build + Test (Canonical)

The Docker image is the canonical build environment — matches CI and production exactly. If Phase 1 passes but Phase 2 fails, there's an env divergence (toolchain version, system lib, OS-specific code path). That's a real bug worth fixing, not a false alarm.

```bash
# Full build + test inside Docker (spins up infra, tears down after)
make docker-test
```

If `make docker-test` is not available or you need finer control:

```bash
# Build the kenjaku image (validates Dockerfile + Rust compile in container)
docker compose -p kenjaku build kenjaku

# Run tests inside the built image
docker compose -p kenjaku run --rm kenjaku sh -c "
  cargo fmt --all -- --check &&
  cargo clippy --workspace --all-targets -- -D warnings &&
  cargo test --workspace
"
```

**Pass criteria:**
- Docker build: 0 errors
- In-container clippy: 0 warnings
- In-container fmt: 0 diffs
- In-container tests: 0 failures (test count should match Phase 1 — divergence is a finding)

**On failure:** Stop and report. If Phase 1 passed but Phase 2 failed, explicitly call out the env divergence in the report.

## Phase 3: Semgrep Guardrail

Semgrep runs on the **host** against source files (not inside Docker — the rule scans `.rs` source, not compiled artifacts).

```bash
# Tenant-scope rule on production SQL code
semgrep --config .semgrep/tenant-scope.yml --error crates/kenjaku-infra/src/postgres/

# Self-test fixtures (verify rule still fires on bad examples)
semgrep --config .semgrep/tenant-scope.yml .semgrep/test/ 2>&1
```

**Pass criteria:**
- Production scan: 0 findings
- Fixture scan: exactly 2 findings (in `bad_missing_tenant.rs` + `bad_get_by_id_no_tenant.rs`)

**On failure:** If production scan finds issues, report as **CRITICAL** — a new tenant-blind query was introduced. If fixtures don't fire as expected, the rule itself may have regressed.

## Phase 4: Docker Deploy + API E2E

### 4a. Deploy (rebuild + restart the running stack)

```bash
# Ensure dev JWT is provisioned (tenancy-first requires it)
test -f config/dev/public.pem || make dev-setup

# Check running containers
docker compose -p kenjaku ps

# Rebuild + restart kenjaku with the latest code
docker compose -p kenjaku up -d --build kenjaku

# If geto-web also changed, rebuild it too (--no-deps to avoid Rust rebuild)
# docker compose build geto-web && docker compose up -d --no-deps geto-web
```

Wait for healthy state:
```bash
# Poll /ready (up to 30s)
for i in $(seq 1 10); do
  curl -sf http://localhost:18080/ready && break
  sleep 3
done
```

### 4b. Startup log verification

```bash
docker compose -p kenjaku logs --tail 30 kenjaku 2>&1 | grep -E "TenantsCache|JWT validator|SessionHistoryStore|Starting|Server listening|Qdrant public alias"
```

**Verify in the log output:**
- `TenantsCache loaded, tenant_count=N` (N >= 1)
- `JWT validator constructed` (tenancy-first — always present)
- `Qdrant public alias ensured` (collection alias active)
- `Server listening` on expected port
- `SessionHistoryStore janitor starting`
- NO `ERROR` / `PANIC` / `FAILED` lines

### 4c. API endpoint smoke tests

All API calls require a valid JWT (tenancy-first architecture). Read the dev token:

```bash
DEV_JWT=$(cat config/dev/dev-token.txt)
SESSION_ID="regression-$(date +%s)"

# 1. Health + readiness (no auth required for these)
curl -sf http://localhost:18080/health | head -c 200
curl -sf http://localhost:18080/ready | head -c 200

# 2. POST /search (requires JWT)
curl -s -X POST \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $DEV_JWT" \
  -H "X-Session-Id: $SESSION_ID" \
  -d '{"query":"what is kenjaku","top_k":3}' \
  http://localhost:18080/api/v1/search \
  --max-time 30 | head -c 500

# 3. GET /autocomplete (requires JWT, tests locale stickiness)
curl -s "http://localhost:18080/api/v1/autocomplete?q=he&limit=3" \
  -H "Authorization: Bearer $DEV_JWT" \
  -H "X-Session-Id: $SESSION_ID" \
  --max-time 10

# 4. GET /top-searches (requires JWT)
curl -s "http://localhost:18080/api/v1/top-searches?limit=5" \
  -H "Authorization: Bearer $DEV_JWT" \
  -H "X-Session-Id: $SESSION_ID" \
  --max-time 10

# 5. Verify unauthenticated requests are rejected
curl -s -X POST \
  -H 'Content-Type: application/json' \
  -d '{"query":"test","top_k":1}' \
  http://localhost:18080/api/v1/search \
  --max-time 5
```

**Pass criteria for each endpoint:**
- Authenticated endpoints: HTTP 200 with `"success": true`
- `/autocomplete` returns `resolved_locale_source` (proves locale chain works)
- `/search` returns non-empty `answer` or `components` array
- Unauthenticated `/search`: returns `"success": false` with `"Unauthorized tenant"` (proves tenancy-first enforcement)

### 4d. Error scan (post-smoke)

```bash
docker compose -p kenjaku logs --since 2m kenjaku 2>&1 | grep -iE "error|panic|failed"
```

**Pass criteria:** Zero matches (warnings from external crates like qdrant version mismatch are acceptable — flag but don't fail).

## Phase 5: Chrome-CDP Web UI Verification

**Skip if Chrome debugger is not available** (check via `scripts/cdp.mjs list`).

### 5a. Find or open geto-web tab

```bash
# List tabs
scripts/cdp.mjs list

# If no localhost:3000 tab, open one
scripts/cdp.mjs open http://localhost:3000
```

### 5b. Navigate + take baseline screenshot

```bash
scripts/cdp.mjs nav <target> http://localhost:3000
scripts/cdp.mjs shot <target> /tmp/regression-baseline.png
```

### 5c. Drive a search

```bash
# Click the search input
scripts/cdp.mjs click <target> 'textarea, input[type="text"], .search-input'

# Type a query
scripts/cdp.mjs type <target> 'What is Bitcoin?'

# Submit (press Enter via eval)
scripts/cdp.mjs eval <target> 'document.querySelector("textarea, input[type=text]")?.form?.requestSubmit() || document.querySelector("textarea, input[type=text]")?.dispatchEvent(new KeyboardEvent("keydown", {key: "Enter", bubbles: true}))'

# Wait for response (SSE streaming takes a few seconds)
sleep 5

# Take result screenshot
scripts/cdp.mjs shot <target> /tmp/regression-search-result.png
```

### 5d. Verify search result

```bash
# Check accessibility tree for answer content
scripts/cdp.mjs snap <target> --compact

# Check for error messages in the page
scripts/cdp.mjs eval <target> 'document.body.innerText.includes("error") || document.body.innerText.includes("Error")'
```

**Pass criteria:**
- Screenshot shows answer text (not a blank or error screen)
- Accessibility snap contains answer content
- No JavaScript errors in console

### 5e. Verify suggestion pills

```bash
scripts/cdp.mjs eval <target> 'document.querySelectorAll(".suggestion-pill, [data-suggestion]").length'
```

**Pass criteria:** At least 1 suggestion pill visible (proves suggestion blending + locale stickiness works end-to-end).

## Phase 6: Worker Health Log Scan

Wait for at least one trending flush cycle (5 minutes), then verify workers are healthy:

```bash
# Wait and scan (or check existing logs if container has been up > 5 min)
docker compose -p kenjaku logs --since 6m kenjaku 2>&1 | grep -E "Flushed trending|flush_once|conversation flush|suggestion refresh"
```

**Pass criteria:**
- At least 1 `Flushed trending entries` log with `flushed=N` (N >= 0, no error)
- `conversation flush` worker started (no crash)
- `suggestion refresh` sleeping until next fire OR actively running (no crash)
- Zero `Trending flush failed` or `flush error` lines

**Note:** If container uptime < 5 min, report Phase 6 as SKIPPED with note "container too fresh for flush cycle — re-run after 5 min or check manually."

## Reporting

After all phases, produce a summary table:

```
## Regression Results — {date} {time}

| Phase | Result | Details |
|-------|--------|---------|
| 1. Local Rust build/test | PASS/FAIL | {test_count} passed, {fail_count} failed |
| 2. Docker build/test | PASS/FAIL | {test_count} passed, {fail_count} failed (env-divergence flag if Phase 1 passed) |
| 3. Semgrep guardrail | PASS/FAIL | {finding_count} findings on production code |
| 4. Docker deploy + API e2e | PASS/FAIL/SKIP | {endpoint_count}/5 endpoints OK + auth rejection verified |
| 5. Chrome-CDP UI | PASS/FAIL/SKIP | search + suggestions verified |
| 6. Worker health | PASS/FAIL/SKIP | {flush_count} flush cycles clean |

Overall: **PASS** / **FAIL** (list failing phases)
```

If any phase FAILs, clearly list the failing check with the exact error output. Do NOT summarize failures — show the raw output so the developer can diagnose without re-running.

## Tips

- Run `/regression quick` for a fast pre-commit check (~1-2 min — phases 1+3: local cargo build/test + semgrep, no Docker)
- Run `/regression api` before opening a PR (~5 min — phases 1-4 with docker rebuild + deploy, no chrome)
- Run `/regression full` before merging or after a significant refactor (~7-10 min — all 6 phases)
- Phase 5 (chrome-cdp) requires Chrome with remote debugging enabled
- Phase 6 (worker health) requires the container to have been up for at least 5 minutes for flush coverage
- If docker isn't running, use `make docker-up` first
- Dev JWT must be provisioned: `make dev-setup` (auto-generates keypair + token)
