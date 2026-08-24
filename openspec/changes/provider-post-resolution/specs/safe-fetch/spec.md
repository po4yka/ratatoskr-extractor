# safe-fetch Delta

## ADDED Requirements

### Requirement: Per-host pacing bounds the request rate

The extractor SHALL enforce a configurable minimum spacing between outbound request starts toward
the same host, in addition to the finite global and per-host concurrency limits, so repeated
fetches cannot exceed the configured per-host rate. Pacing SHALL operate within the caller's
absolute operation deadline and SHALL NOT extend it.

#### Scenario: rapid successive requests are spaced

- **WHEN** two fetches toward the same host start closer together than the configured spacing
- **THEN** the second request begins no earlier than the configured interval after the first while
  remaining within its operation deadline

#### Scenario: pacing cannot exceed the deadline

- **WHEN** a fetch's remaining operation allowance is shorter than the pending host spacing
- **THEN** the fetch fails with its deadline error rather than waiting past the deadline
