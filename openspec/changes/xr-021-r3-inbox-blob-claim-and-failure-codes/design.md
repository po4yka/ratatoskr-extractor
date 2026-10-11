## Context

Cross-repository decisions live in XR-021 CONTRACTS.md (R3-01 inbox prefixes, R3-14 run-level claim and public failure codes, R3-19 stale documentation). This design records only the extractor-internal choices.

## Decisions

### D1. The prefix is applied where the options are consumed

`NatsPublisher::connect_with_options` applies `custom_inbox_prefix(INBOX_PREFIX)` to the options it receives, so `connect`, `connect_with_nkey` and every caller that passes its own authentication (the administrators of the tests) get it and no caller can forget it. The capture consumer and the render-await consumer use the publisher's `jetstream::Context`, so one connection carries both. The worker builds its options in one private function that `connect` calls, with or without a seed, and the KV bucket handle comes from that client.

### D2. The claim is a property of the run

The three claim columns move from `extractor.sources` to `extractor.extraction_runs`, nullable and constrained together, because a URL run has no blob claim. `blob_source` keeps inserting the digest identity (`owner_id`, `normalized_url`, `canonical_url`) and `queue_run` writes the command's claim on the new run in the same intake transaction. `claim_queued_run` joins nothing new: it reads the claim from the run it claims. Two commands for one digest queue two runs, each judged against its own claim.

### D3. One function maps a class to a public code

`public_failure(class) -> (ErrorCode, SafeMessage)` sits beside `enqueue_failed_report`, which takes the class and calls it. The match is closed over the classes of `docs/INTERFACES.md` with `content.extraction.failed` as the default for every other class, so an unknown class never leaks its name. `retryable` stays the value the caller passes.

### D4. The operator example is the test

`browser-worker.conf.example` is an EnvironmentFile. The test parses its `KEY=value` lines into the figment jail and loads `WorkerSettings`, so a key the settings do not know fails the test (`deny_unknown_fields`).
