# Inputs

`s2-cli` reads inputs from:

1. **CLI arguments:** Subcommands and positional arguments parsed via `clap`. The `verify` and `health` subcommands accept an optional name or substring filter.
2. **Environment variables:** Loads from `.env.local` (walked up from CWD). Recognized vars:
   - `RAILWAY_TOKEN` — Railway GraphQL access for remote deploy status
   - `S2_CONTROL_PLANE_PATH` — override path to `control-plane.json`
3. **`control-plane.json`:** Fleet registry read at startup. Declares `live_runtimes`, `scaffolded_runtimes`, `brokers`, and `product_services`. Each entry carries `name`, `port`, `auth`, `tools`, `health_path`, and `probe` fields consumed by `verify`.
4. **Webhooks:** The `watch` subcommand opens a local HTTP listener on the specified port to ingest Railway deployment event payloads.
