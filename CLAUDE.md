# `s2-cli` Development Guidelines

As an AI assistant operating within the `s2-cli` project, you must strictly adhere to the following rules:

## Architectural Invariants
1. **Zero Authority**: `s2-cli` is an operational observability tool. It must never be granted authority to mint tokens, bypass isolation, or directly modify database state.
2. **Boundary Agnostic Observation**: The CLI aggregates data across separated boundaries (Orchestration, Brokers, Runtimes) purely for developer convenience. It must not store, correlate, or persist cross-boundary data locally.
3. **API-First Execution**: Any mutation (like deploying a service) must happen exclusively through established provider pipelines (e.g., invoking `railway` CLI or hitting Railway GraphQL endpoints).
4. **Rust Idioms**: Maintain a lightweight and fast execution profile. Use `clap` for arguments, `tokio` for async orchestration, and strictly handle errors rather than silently failing (refer to `FAILURE_BEHAVIOR.md`).
