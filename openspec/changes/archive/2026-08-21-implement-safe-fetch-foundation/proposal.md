## Why

`ratatoskr-extractor` can safely fetch and retain public source bytes, but those bytes are not yet
converted to shared Document IR and Platform capture commands still have no consumer. The next slice
must add parse-once document construction and the durable persistence/event boundary without pulling
candidate scoring from plan item 5 into the work.

## What Changes

- Add the Rust workspace, one extractor service, typed environment configuration, structured telemetry, typed errors, admin health endpoints, a reusable local test harness, CI gates, and a hardened systemd unit based on `ratatoskr-platform`.
- Add deterministic URL normalization and source classification while preserving the original address.
- Add mandatory SSRF policy for the initial target, every resolved address, and every redirect target.
- Add one bounded streaming HTTP fetch path with explicit redirect handling, cache validators, response metadata, incremental SHA-256, and immutable raw-artifact storage.
- Return the published `BlobRef` contract for stored bytes without introducing a blob service, database, event bus, or Document IR.
- Parse bounded HTML once and construct the published Document IR deterministically from the shared DOM.
- Add the extractor-owned PostgreSQL schema, durable run/artifact/candidate records, transactional inbox/outbox, JetStream command consumption, result publication, and operation reports.

## Capabilities

### New Capabilities

- `service-foundation`: Process configuration, telemetry, health, shutdown, test support, CI, and deployment behavior for the first extractor service.
- `safe-url-routing`: Deterministic URL normalization, source classification, and SSRF decisions before network access.
- `safe-fetch`: Bounded streaming retrieval, redirect revalidation, cache metadata, and extractor-owned content-addressed raw artifacts.
- `document-ir`: Bounded parse-once HTML conversion into the shared Document IR contract.
- `event-pipeline`: Extractor-owned persistence and at-least-once command/event processing.

### Modified Capabilities

None.

## Impact

- Adds the first Rust workspace members and the `ratatoskr-extractor` deployable.
- Adds pinned Rust dependencies for async HTTP, TLS, configuration, telemetry, serialization, hashing, errors, and the shared `BlobRef` contract.
- Adds `.github/workflows/ci.yml`, the matching command list in `DEVELOPMENT.md`, and root Rust tool/lint/policy files.
- Adds `deploy/systemd/ratatoskr-extractor.service` and its documented environment contract.
- Adds one editable `schema.sql`, not migration tooling, and copies Platform's PostgreSQL/JetStream
  delivery pattern under the extractor-owned schema.
- Does not add candidate extraction/scoring, PDF/provider adapters, browser code, or a blob service.
