## Why

Round 3 of changeset XR-021 re-verified four extractor defects against `origin/main` (`cf64d84`). Both NATS connections of the extractor use the default `_INBOX.` reply prefix, so the ACL has to let the identity subscribe to the shared `_INBOX.>` namespace and a compromised service could read the JetStream deliveries of every other service. A blob capture is verified against the claim of the first command ever seen for its digest, because the claim lives on the shared `extractor.sources` row: a wrong-length first capture poisons every later corrected capture, and a re-capture with a wrong media type or length succeeds for any digest seen before. Every failed run reaches the user as `content.extraction.failed`, so a scanned PDF cannot be told from a parser fault. The browser worker has no loadable operator example, and the README still says that no workspace pin exists. The behaviour is specified fleet-wide by XR-021 CONTRACTS.md sections R3-01 and R3-14 (and R3-19 for the README, R2-10 rule 2 for the example); this change implements the extractor half.

## What Changes

- **BREAKING (behaviour, no shim)**: both connections use their own inbox prefix, `_INBOX_extractor` for the capture consumer and the outbox publisher (one `INBOX_PREFIX` constant in `crates/eventing`, applied inside `NatsPublisher::connect_with_options`, the one place that builds the options) and `_INBOX_extractor_browser_worker` for the worker (applied inside `browser_worker::connect`, KV bucket handle included). `deploy/nats/identity.conf` and `identity-browser-worker.conf` subscribe to exactly that prefix. A service built before this round cannot complete JetStream requests under the new ACL; Platform's `ratatoskr.conf` changes in the same round and the services of one snapshot deploy together.
- **BREAKING (schema, no migration)**: `extractor.extraction_runs` gains nullable `blob_owner`, `blob_media_type`, `blob_length_bytes` (all three or none, the bounds of the former source checks); `extractor.sources` keeps only the digest identity. `queue_run` writes the claim on the run and `claim_queued_run` reads it from the run. Development databases are recreated from `schema.sql`.
- **BREAKING (contract text)**: a failed run reports one of five public codes chosen from its closed failure class (`content.extraction.no_text_layer`, `encrypted`, `unsupported_media`, `source_unavailable`, and the unchanged `failed`). `enqueue_failed_report` takes the class; the class names stay operator-only.
- New `deploy/systemd/browser-worker.conf.example` that loads through `WorkerSettings::load`, and a "Browser worker" paragraph in `deploy/README.md`.
- `README.md` states the workspace pins and the harness checks as they are, and its "Bus identity" paragraph names the inbox prefix.

## Capabilities

### New Capabilities

- `own-inbox-prefix`: each extractor connection replies on its own inbox prefix and its stanza subscribes to nothing else.
- `run-level-blob-claim`: a blob run verifies against the claim of its own command.
- `public-failure-codes`: a failed run reports a public code from a closed set.

### Modified Capabilities

(none; `openspec/specs/` has no matching capability today)

## Impact

- Code: `crates/eventing` (outbox, capture, completion), `crates/persistence` schema consumers, `services/extractor` (pdf run), `services/browser-worker` (connect), tests for each, `schema.sql`.
- Contracts: no change. Stays on ratatoskr-contracts `ad16855c4e7f3d52cd118274faa3b8f3ab4da576`. Cross-repository behaviour is defined in XR-021 CONTRACTS.md sections R3-01 and R3-14; this change cites them and restates only what the extractor must do.
- Docs: `README.md`, `deploy/README.md`, `docs/INTERFACES.md`, `deploy/systemd/browser-worker.conf.example`.
- Out of scope: URL captures, the render escalation path, a systemd unit for the browser worker, Platform's ACL and the workspace equality check, the Document IR and `content.document.extracted.v1`.
