## Why

The extractor's outbox loses facts and hides a stuck publisher. It dead-letters any row after 12 failed attempts, which is about 23 minutes of backoff, so a broker outage longer than that permanently drops operation reports and `content.document.extracted.v1` events: an operation can stay `running` forever and the document never reaches Knowledge. A document larger than the broker's message limit is retried until it dead-letters. A relay that keeps running but cannot publish does not reach `/health/ready`, because readiness looks only at the database and at whether the NATS client is connected. The behaviour is specified fleet-wide by XR-021 CONTRACTS.md sections R2-04 (failure classes and the payload limit) and R2-03 (publisher health reaches readiness); this change implements the extractor half.

## What Changes

- **BREAKING (behaviour, no shim)**: a transient publish failure never dead-letters a row. `mark_failed` is replaced by `mark_transient` (records the attempt, a safe class and the capped backoff) and `mark_undeliverable` (sets `dead_lettered_at` with `last_error = 'payload_too_large'`). `OutboxReport.dead_lettered` now counts only permanent failures.
- `PublishError` gains `too_large()` and `is_permanent()`. `NatsPublisher::publish` compares the body length with the connected server's `max_payload` minus 1 KiB before publishing, so an oversize message is classed without waiting for an acknowledgement.
- New counters `ratatoskr_extractor_outbox_publications_total{outcome="undeliverable"}` and `ratatoskr_extractor_outbox_undeliverable_total{class}`.
- `OutboxHealth` (in `crates/eventing/src/outbox.rs`) turns repeated refusals into an unhealthy publisher: the last pass attempted a publish, at least one publish failed, and the oldest unpublished publishable row has `attempts >= 3` or is older than 300 seconds. `outbox_loop` feeds it to readiness through `RuntimeHealth`; the loop never stops. An undeliverable row never counts.
- New configuration `bus.max_event_bytes` (`RATATOSKR__BUS__MAX_EVENT_BYTES`, default 8000000, accepted 65536..=8388608). Completion serializes the `ContentDocumentExtracted` envelope before choosing the rows; over the limit, no document event is enqueued and the operation report is `partially_succeeded` with the warning `content.document.not_ingested` instead of `succeeded`.
- **BREAKING (API, no shim)**: `complete_document` and `complete_blob_document` take the claimed `&QueuedRun` instead of a bare run id, and `claim_queued_run` takes the limit that the run carries.

## Capabilities

### New Capabilities

- `outbox-failure-classes`: transient failures retry forever, permanent failures are undeliverable at once, repeated refusals reach readiness, and an oversize document is reported truthfully.

### Modified Capabilities

(none; `openspec/specs/` has no outbox capability today)

## Impact

- Code: `crates/eventing` (outbox, completion, capture claim), `crates/core` (config), `services/extractor` (outbox loop, readiness, completion call sites), tests for each.
- Contracts: no change. Stays on ratatoskr-contracts `ad16855c4e7f3d52cd118274faa3b8f3ab4da576`. Cross-repository behaviour is defined in XR-021 CONTRACTS.md sections R2-04 and R2-03; this change cites them and restates only what the extractor must do.
- Docs: `README.md`, `deploy/README.md`, `deploy/systemd/extractor.conf.example`.
- Residual risk: a refused publish is visible only as a `Publish Violation` in the NATS server log; the extractor can only report the symptom (a missing acknowledgement).
