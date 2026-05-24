# Failure Behavior

If `s2-cli` encounters failures during execution:

1. **Missing `control-plane.json`:** Exits immediately with a clear error. Set `S2_CONTROL_PLANE_PATH` to override the search path.
2. **Missing `RAILWAY_TOKEN`:** Railway columns show `—` (dimmed). Local liveness checks proceed normally. No crash.
3. **Partial API failures:** If Railway GraphQL returns an error for one service, that row shows `gql-error` in the Railway column. Other services continue.
4. **`verify` — no matching services:** Exits `1` with an explicit error if the filter string matches nothing in `live_runtimes` or `product_services`.
5. **`verify` — service not running locally:** Liveness shows `✗ connection refused`. Auth-gate and manifest checks show `– service not running` (skip, not fail). Only liveness counts as a failure for services that are simply not started.
6. **`verify` — auth-gate unexpected response:** A 200 (auth bypassed) or 5xx (middleware panic) is a hard failure, reported as `✗` with the status code and a description of which failure class it represents.
7. **`verify` — manifest returns non-JSON:** Reported as `✗` even if the HTTP status was 200. A 404 is a skip, not a failure.
8. **Webhook port contention:** If the port is already bound when running `watch`, exits with an OS bind error. Free the port or pass `--port <N>` with an alternative.
