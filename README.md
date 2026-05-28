# S2Forge Systems CLI (`s2`)

Native Rust CLI for unified operational observability across the S2Forge fleet. Reads from `control-plane.json` and Railway GraphQL; never writes to production state.

## Purpose

In a boundary-native architecture, services are physically separated to prevent collapse. `s2-cli` aggregates status and runs behavioral checks across that separation without holding authority or correlating identities across boundaries.

## Setup

1. Ensure the Railway CLI is installed if using Railway-backed status checks.
2. Place tokens in `.env.local` at the repo root (walked up from CWD):
   ```env
   RAILWAY_TOKEN=...
   ```

## Commands

### Status

Color-coded table of all services — brokers, live runtimes, and scaffolded runtimes — with Railway deploy status and local liveness.

```bash
cargo run -- status
```

### Brokers / Runtimes

Scoped views of the fleet.

```bash
cargo run -- brokers
cargo run -- runtimes
```

### Health

Deep ping of a single named service: hits `/health`, pretty-prints the response body, shows HTTP status.

```bash
cargo run -- health text-runtime
cargo run -- health coach-broker
```

### Worktree

Manage git worktrees for the monorepo-per-category repos (the **clean-main-is-sacred**
discipline — see `/WORKFLOW.md`). Keeps each category's `main` checkout clean and deployable
while in-progress work lives in `.wt/<category>--<name>` beside it.

```bash
s2 worktree add generation vision-fix      # new branch feat/vision-fix in .wt/generation--vision-fix
s2 worktree add compute --at <sha>         # detached worktree at a commit (hotfix on the running SHA)
s2 worktree ls                             # list linked worktrees across all category repos
s2 worktree rm generation vision-fix       # remove + prune
```

On `add` it creates a shared per-category build-cache dir and prints the `CARGO_TARGET_DIR`
export and `railway link` command to run in the new worktree.

### Verify

Behavioral smoke tests — runs three checks against each matched service concurrently and exits non-zero if any fail. Intended for post-deploy validation.

```bash
# All live runtimes
cargo run -- verify

# Filter by name or product substring
cargo run -- verify coach
cargo run -- verify book
cargo run -- verify fleet
cargo run -- verify text-runtime
```

**Checks per service:**

| Check | Probe | Pass condition |
|---|---|---|
| `liveness` | `GET <health_path>` | 2xx |
| `auth-gate` | `POST /v1/<tool>` without a token | 401 or 403 |
| `manifest` | `GET /v1/control/manifest` | 2xx + valid JSON (404 = skip) |

The `auth-gate` check catches two failure classes: a 200 means auth is bypassed; a 5xx means the auth middleware is panicking on unauthenticated requests. Both are regressions.

Exit code is `0` if all checks pass, `1` if any fail.

### Gaps

Lists all services in `control-plane.json` with non-empty `gaps[]` entries.

```bash
cargo run -- gaps
```

### Watch

Local HTTP server that ingests Railway deployment webhook payloads. Pipe through a tunnel (e.g., ngrok) to watch real-time deploy events.

```bash
cargo run -- watch --port 4000
```

## Service registry

All service discovery is driven by `Systems/Runtimes/control-plane.json`. Override the path:

```env
S2_CONTROL_PLANE_PATH=/path/to/control-plane.json
```

Product-tier services (Book, Coach, Fleet) are declared under `product_services[]` in the same file.
