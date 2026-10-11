# outbox-failure-classes Delta

## ADDED Requirements

### Requirement: A transient publish failure never loses a row

A publish failure that is not classed permanent SHALL record the attempt, a safe error class and the capped backoff, and SHALL NOT set `dead_lettered_at`, however many attempts have failed. The row SHALL publish on the first pass that succeeds.

#### Scenario: thirty consecutive failures

- **WHEN** the publisher fails thirty consecutive passes for one row and then succeeds
- **THEN** the row is unpublished and not dead-lettered with `attempts = 30` after the failures, and published on the first successful pass

### Requirement: An oversize message is undeliverable at once

A message whose serialized body exceeds the connected broker's `max_payload` minus 1 KiB SHALL be classed `PublishError::too_large()` before publishing, and its row SHALL be marked undeliverable after one attempt with `last_error = 'payload_too_large'`, without delaying later rows in the same pass.

#### Scenario: an oversize row among deliverable rows

- **WHEN** the head row is oversize and a later row is small
- **THEN** the head row has `dead_lettered_at` set after one attempt and the later row is published in the same pass

#### Scenario: a body larger than the broker limit

- **WHEN** a 128 KiB body is published through `NatsPublisher` against a broker with `max_payload: 65536`
- **THEN** the typed too-large error returns without waiting for a PubAck

### Requirement: Publisher health reaches readiness

`/health/ready` SHALL be 503 when the last outbox pass attempted a publish, at least one publish failed, and the oldest unpublished publishable row has `attempts >= 3` or is older than 300 seconds. It SHALL be 200 again after a pass that publishes every due row or finds none. An undeliverable row SHALL NOT count, and the unhealthy state SHALL NOT stop the relay.

#### Scenario: a refused subject

- **WHEN** the broker refuses the operation report subject and a row is queued
- **THEN** readiness turns 503 after the third refused attempt, `/metrics` shows the failure counter, and readiness returns to 200 when the broker allows the subject

### Requirement: An oversize document is reported truthfully

Completion SHALL NOT enqueue `evt.content.document.extracted.v1` when the serialized envelope exceeds `bus.max_event_bytes` (default 8000000, accepted 65536..=8388608). The operation report SHALL then be `partially_succeeded` with the warning `content.document.not_ingested` ("The document was extracted, but it is too large to add to the library.") and SHALL keep `results[content.document]`.

#### Scenario: over and under the limit

- **WHEN** a run completes with an envelope above the limit, and another with one below it
- **THEN** the first enqueues one `partially_succeeded` report and no document event, and the second enqueues the document event and a `succeeded` report
