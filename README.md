# S2Forge Systems CLI (`s2`)

A dedicated native Rust CLI tool designed for unified operational management and observability across the separated S2Forge boundary environments (Orchestration, Brokers, Runtimes).

## Purpose
In a strict boundary-native architecture, infrastructure is physically separated to prevent collapse. While this is architecturally sound, it introduces operational friction when managing deployments. `s2-cli` abstracts this friction by orchestrating the Railway CLI underneath, giving you a unified view without compromising system boundaries.

## Setup

1. Ensure the Railway CLI is installed.
2. The CLI expects your tokens to be defined in `.env.local` at the root of `Systems/`:
   ```env
   RAILWAY_ORCHESTRATION_TOKEN=...
   RAILWAY_BROKERS_TOKEN=...
   RAILWAY_RUNTIMES_TOKEN=...
   ```

## Usage

You can run the CLI via Cargo.

### Check Statuses
Queries all three environments concurrently and returns a color-coded table of service deployments and their statuses.
```bash
cargo run -- status
```

### Tail Logs
Specify the name of the service (e.g., `text-runtime`). The CLI will dynamically resolve which project owns it and tail the logs.
```bash
cargo run -- logs text-runtime
```

### Watch Deployment Events
Starts a local HTTP server to ingest webhook payloads from Railway natively. Useful when piped through a local tunnel (e.g., ngrok) to watch real-time deployments.
```bash
cargo run -- watch --port 4000
```
