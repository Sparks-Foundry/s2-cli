# s2-cli

## Purpose
`s2-cli` provides local operational mechanisms to test, observe, and manage S2Forge environments across boundary domains without becoming part of the active system authority.

## Authority Restrictions
- `s2-cli` MUST NEVER become an authority surface.
- `s2-cli` MAY observe systems (e.g., fetching logs, deployment statuses).
- `s2-cli` MAY NOT silently compose identity or bypass plane isolation.
- Mutations (like triggering a deployment) must be strictly mapped to valid provider operations (e.g., via Railway API).
