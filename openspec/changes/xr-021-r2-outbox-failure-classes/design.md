## Context

Cross-repository decisions live in XR-021 CONTRACTS.md (R2-04 failure classes and the 8 MiB broker limit, R2-03 publisher health). This design records only the extractor-internal choices.

## Decisions

### D1. Classes decide the row, not the attempt count

A publish error is transient unless `PublishError::is_permanent()`. A transient failure runs `mark_transient`: attempts plus one, a safe `last_error`, claim released, `next_attempt_at` advanced by `least(300, 2^attempts)` seconds. It never sets `dead_lettered_at`, so the 12-attempt ceiling disappears; an outage of any length ends with the row published. A permanent failure runs `mark_undeliverable`: attempts plus one, `last_error = 'payload_too_large'` (a fixed safe class, never the error text), `dead_lettered_at` set. The pass continues with the next claimed rows, so an undeliverable head row never starves later ones. A body that cannot be serialized from the stored JSON is also permanent, with the class `payload_unserializable`. The column keeps its name `dead_lettered_at`; the schema already carries the exclusivity check and the partial index.

### D2. The size is checked against the broker that is connected

`NatsPublisher::publish` reads `client.server_info().max_payload` and refuses a body longer than that minus 1024 bytes before any network round trip. The check returns `PublishError::too_large()`, which is a typed variant, not a string match. A broker whose limit is lower than `bus.max_event_bytes` is therefore still handled, as the backstop for the enqueue-time guard.

### D3. OutboxHealth is computed by the pass, not by the loop

`run_outbox_once` returns `OutboxReport` plus the facts needed by the rule: whether a publish was attempted, whether one failed, and the oldest unpublished publishable row's `attempts` and age (one query per failing pass). `OutboxHealth::observe(&report)` is a pure function over those facts: unhealthy when a publish failed and the head row has `attempts >= 3` or is older than 300 seconds; healthy after a pass that attempted nothing or published everything due. Undeliverable rows are excluded from the head-row query because they carry `dead_lettered_at`. `outbox_loop` owns one `OutboxHealth`, passes the verdict to `RuntimeHealth` as a separate named flag so a healthy database probe cannot overwrite it, and increments a labelled counter when the verdict flips. The loop never returns because of it.

### D4. The enqueue guard decides the status

`complete` serializes the document event envelope once, compares its length with the run's `max_event_bytes`, and only then chooses the rows: under the limit, both rows and `succeeded`; over it, the report row only, `partially_succeeded`, one `content.document.not_ingested` warning, and `results[content.document]` still present with the IR blob, because the document was extracted. The decision and the rows share one transaction. The limit travels on `QueuedRun`, set by `claim_queued_run`, so every completion path reads the policy of the process that claimed the run and `complete_document` stays at seven parameters.
