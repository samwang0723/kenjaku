---
name: rebuild-docker
description: Conditionally rebuild the kenjaku service and/or geto-web frontend in docker-compose, choosing the minimal rebuild based on which files changed since the running containers were started.
---

# rebuild-docker

Rebuild only what needs rebuilding. Touching `geto-web/index.html` should not
trigger a 2-minute Rust rebuild. Touching `crates/kenjaku-service/src/search.rs`
should not redeploy the static site.

## How to decide what to rebuild

Compare the modification time of relevant files to the start time of each
running container. A file is "newer" if it was modified after its container
started.

```bash
# Container start times (RFC3339)
KENJAKU_STARTED=$(docker inspect -f '{{.State.StartedAt}}' kenjaku-kenjaku-1 2>/dev/null || echo "1970-01-01T00:00:00Z")
GETO_STARTED=$(docker inspect -f '{{.State.StartedAt}}' kenjaku-geto-web-1 2>/dev/null || echo "1970-01-01T00:00:00Z")
```

Then categorize candidate files into two buckets:

### Rebuild **kenjaku** when any of these are newer than `KENJAKU_STARTED`:
- `crates/**/*.rs` — any Rust source
- `crates/**/Cargo.toml` — dependency changes
- `Cargo.toml` / `Cargo.lock` — workspace
- `Dockerfile` — kenjaku image build
- `migrations/**` — schema changes (rebuild not strictly required, but the
  startup migrator runs at container start, so a restart is enough — see
  the "config-only" branch below)

### Rebuild **geto-web** when any of these are newer than `GETO_STARTED`:
- `geto-web/index.html`
- `geto-web/app.js`
- `geto-web/styles.css`
- `geto-web/enso.svg`
- `geto-web/nginx.conf`
- `geto-web/Dockerfile`
- `geto-web/fonts/**`

### Restart only (no rebuild) when only these changed:
- `config/base.yaml`, `config/{env}.yaml` — kenjaku reads config from a
  bind-mounted path, so a `docker compose restart kenjaku` is enough

### Both changed
Run them in parallel-ish — `docker compose up -d --build kenjaku geto-web`.

### Nothing changed
Print "Nothing to rebuild" and exit.

## Steps

1. **Detect the working directory** — must be the project root (where
   `docker-compose.yaml` lives). If not, abort with a clear error.

2. **Check container state**:
   ```bash
   docker compose ps --format '{{.Service}} {{.State}}'
   ```
   If neither service is up, the right command is `make docker-up`, not this
   skill. Tell the user that and exit.

3. **Compute the change set** using `find` with `-newer` against a temp file
   stamped to the container start time. Example for kenjaku:
   ```bash
   STAMP=$(mktemp)
   touch -d "$KENJAKU_STARTED" "$STAMP"
   find crates Cargo.toml Cargo.lock Dockerfile -newer "$STAMP" -type f 2>/dev/null
   rm -f "$STAMP"
   ```
   (Same pattern for geto-web with the geto-web file list.)

4. **Print a summary** of which service(s) will be rebuilt and why, listing
   1–5 sample changed files so the user can sanity-check.

5. **Ask before running** if both services or kenjaku alone is about to be
   rebuilt (kenjaku build is slow). For geto-web alone, just go.

6. **Run the minimal rebuild**:
   - geto-web only: `docker compose up -d --build geto-web`
   - kenjaku only: `docker compose up -d --build kenjaku`
   - both: `docker compose up -d --build kenjaku geto-web`
   - config-only: `docker compose restart kenjaku`

7. **Verify health** after the rebuild:
   ```bash
   sleep 3
   docker compose ps
   curl -sf http://localhost:18080/ready | head -c 200
   curl -sfI http://localhost:3000/ | head -1
   ```

## Notes

- Use the existing Makefile targets where they exist:
  `make geto-web-up` → `docker compose up -d --build geto-web` (same thing)
- The skill should be a no-op if the project hasn't been started at least
  once — bail with `make docker-up` instructions instead.
- Never `docker compose down` followed by `up` — that drops Qdrant data.
  Always use targeted `up -d --build <service>` so the data containers
  stay running.
- If `kenjaku` fails to start after rebuild, dump the last 30 lines of logs:
  `docker compose logs --tail 30 kenjaku`.
- For slow Rust rebuilds, mention that the user can watch progress with
  `make docker-logs` in another terminal.
