# Outputs

`s2-cli` produces outputs in the following formats:

1. **Terminal (stdout/stderr):** Human-readable, colorized output for all commands. Green = healthy/passing, red = failed, yellow = degraded/warning, dim = skipped or not applicable.
2. **Exit codes:**
   - `0` — success; all checks passed (relevant to `verify`)
   - `1` — one or more checks failed, or a fatal error occurred
   - All other commands exit `0` unless a hard error (e.g., missing `control-plane.json`) prevents execution.
3. **Webhook echo (stdout):** The `watch` subcommand pretty-prints each inbound Railway deployment payload to stdout as it arrives.
