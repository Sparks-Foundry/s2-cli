# Failure Behavior

If `s2-cli` encounters failures during execution:

1. **Missing Environment Tokens:** Fails fast and prompts the user to verify `.env.local` contents. It will not attempt fallback logic for authentication.
2. **Partial API Failures:** If one environment (e.g., Brokers) fails to return status, `s2-cli` logs the specific environment's error and continues aggregating the other environments.
3. **Webhook Port Contention:** If the port is already bound when running `watch`, it will gracefully error with instructions to free the port or provide an alternative.
