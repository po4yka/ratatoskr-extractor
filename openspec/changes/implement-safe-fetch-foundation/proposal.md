## Why

`ratatoskr-extractor` has no product code, so Platform capture commands still have no safe path to obtain and retain public source bytes. The first implementation slice must establish the service runtime and the security boundary around URL handling, network retrieval, and raw artifacts before parsing or event-bus integration starts.

## What Changes

- Add the Rust workspace, one extractor service, typed environment configuration, structured telemetry, typed errors, admin health endpoints, a reusable local test harness, CI gates, and a hardened systemd unit based on `ratatoskr-platform`.
- Add deterministic URL normalization and source classification while preserving the original address.
- Add mandatory SSRF policy for the initial target, every resolved address, and every redirect target.
- Add one bounded streaming HTTP fetch path with explicit redirect handling, cache validators, response metadata, incremental SHA-256, and immutable raw-artifact storage.
- Return the published `BlobRef` contract for stored bytes without introducing a blob service, database, event bus, or Document IR.

## Capabilities

### New Capabilities

- `service-foundation`: Process configuration, telemetry, health, shutdown, test support, CI, and deployment behavior for the first extractor service.
- `safe-url-routing`: Deterministic URL normalization, source classification, and SSRF decisions before network access.
- `safe-fetch`: Bounded streaming retrieval, redirect revalidation, cache metadata, and extractor-owned content-addressed raw artifacts.

### Modified Capabilities

None.

## Impact

- Adds the first Rust workspace members and the `ratatoskr-extractor` deployable.
- Adds pinned Rust dependencies for async HTTP, TLS, configuration, telemetry, serialization, hashing, errors, and the shared `BlobRef` contract.
- Adds `.github/workflows/ci.yml`, the matching command list in `DEVELOPMENT.md`, and root Rust tool/lint/policy files.
- Adds `deploy/systemd/ratatoskr-extractor.service` and its documented environment contract.
- Does not add PostgreSQL schema or migration tooling, NATS/outbox/inbox code, browser code, parsing, Document IR, or cross-repository event behavior.
