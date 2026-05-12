# Inputs

`s2-cli` reads inputs from:

1. **CLI Arguments:** Standard arguments and subcommands parsed via `clap`.
2. **Environment Variables:** Loads from `.env.local` to resolve boundary-specific API tokens (e.g., `RAILWAY_ORCHESTRATION_TOKEN`, `RAILWAY_BROKERS_TOKEN`, `RAILWAY_RUNTIMES_TOKEN`, `RAILWAY_WORKSPACE_TOKEN`).
3. **Webhooks:** The `watch` subcommand can open a local HTTP listener to ingest payload events natively from Railway or the Control Plane.
