## 1. Failure classes

- [x] 1.1 Add failing tests in `crates/eventing/tests/outbox.rs`: `transient_failures_never_dead_letter`, `an_oversize_row_is_undeliverable_at_once_and_does_not_block_later_rows`, `a_body_larger_than_the_server_limit_is_classed_before_publishing`. Expected failure today: the transient row dead-letters at attempt 12 and `PublishError::too_large()` does not exist (a signature-only stub that returns a neutral value lets the test compile and fail on its assertion).
- [x] 1.2 Add `PublishError::too_large()` and `is_permanent()`, the size check in `NatsPublisher::publish`, `mark_transient` and `mark_undeliverable`, and the two counters. Verify: 1.1 green.

## 2. Publisher health reaches readiness

- [x] 2.1 Add failing tests `crates/eventing/tests/outbox.rs::repeated_refusals_mark_the_publisher_unhealthy_until_a_clean_pass` and `services/extractor/tests/admin.rs::readiness_follows_the_outbox_publisher`. Expected failure today: no `OutboxHealth` verdict exists and the service stays ready against a broker that refuses the publish.
- [x] 2.2 Add `OutboxHealth`, feed it from `outbox_loop` into `RuntimeHealth`, and count verdict flips. Verify: 2.1 green.

## 3. Enqueue-time size guard

- [x] 3.1 Add failing tests `crates/core/tests/config.rs::bus_max_event_bytes_defaults_and_bounds`, `crates/eventing/tests/completion.rs::an_oversize_document_is_reported_partial_and_no_event_is_enqueued` and the control `a_document_under_the_limit_still_succeeds`. Expected failure today: unknown key under `deny_unknown_fields`, and the document event is always enqueued.
- [x] 3.2 Add `bus.max_event_bytes` with validation, carry it on `QueuedRun`, decide status and rows in `complete`, and document the key. Verify: 3.1 green and the existing suites green.

## 4. Documentation

- [x] 4.1 State the three outbox outcomes, the readiness rule, the 8 MiB broker limit and the `Publish Violation` log line in `README.md` and `deploy/README.md`. Why no failing test first: documentation over delivered work.

## 5. Gate

- [x] 5.1 Run the full gate including `cargo deny --locked check` and `openspec validate --all --strict`. Why no failing test first: verification over delivered work.
