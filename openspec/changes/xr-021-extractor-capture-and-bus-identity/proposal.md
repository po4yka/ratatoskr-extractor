## Why

Four defects in the extractor's bus edge keep the fleet from working end to end, and one operator port collides with another service. They are specified fleet-wide by XR-021 CONTRACTS.md; this change implements the extractor half.

- The capture consumer decodes a legacy flat command document (`CaptureCommandWire`) that no producer will emit after XR-021: Platform and X move `content.capture.requested.v1` to the typed `CommandEnvelope` (CONTRACTS.md section S11), and a command may now name a Telegram-owned PDF blob instead of a URL.
- `content.document.extracted.v1` is published with the flat Document as its payload; the contract is the `ContentDocumentExtracted` wrapper with the extractor-owned IR blob reference (S09).
- The extractor connects with an identity that may create streams and consumers (`evt.>`, `$JS.API.>`). The fleet ACL narrows it to a handful of subjects and the Edge-provisioned durables, so the extractor must verify topology and never create it (S02 rule 6, S03, S04). The browser worker has no credential at all today and creates its stream and KV bucket.
- The operator listener default 9467 collides with the Telegram webhook operator port (S05).

## What Changes

- **BREAKING (contract, no shim)**: `decode_capture` parses the typed `CommandEnvelope` and `ContentCaptureRequested`; `CaptureCommandWire`, `CapturePayload` and `COMMAND_PRODUCER` are deleted; `CaptureCommand.url` becomes `CaptureCommand.source: CaptureSource { Url | Blob }`. Interleaved old and new producers drop commands, so Platform, X and the extractor deploy together.
- **BREAKING (contract, no shim)**: `evt.content.document.extracted.v1` carries `ContentDocumentExtracted { document, document_blob }` instead of the flat Document.
- `extractor.sources` gains a blob source form (`schema.sql` edited in place); a blob capture queues a `pdf-v1` run whose bytes are read from the Telegram-owned blob root, verified, copied into the extractor store and extracted without any network fetch. Failure classes: `blob_owner`, `blob_missing`, `blob_mismatch`, `blob_unreadable`, `unsupported_media`.
- New configuration `blobs.telegram_root` and `bus.provision_topology` (default false, refused with an nkey seed); the deployment unit gains the Telegram blob group and read-only path.
- The command consumer and the render-await consumer fetch their Edge-provisioned durables with `get_consumer_from_stream` and verify them; `ensure_*` calls remain only behind `provision_topology`. `deploy/nats/extractor-permissions.conf` is replaced by `deploy/nats/identity.conf` (the EXTRACTOR stanza of Platform's `ratatoskr.conf`) and `deploy/nats/identity-browser-worker.conf`.
- The browser worker authenticates with an nkey, fetches its durable and the `browser_worker_completions` KV bucket by get only, and creates topology only behind `BROWSER_PROVISION_TOPOLOGY`.
- The operator listener default moves from `127.0.0.1:9467` to `127.0.0.1:9088`.

## Capabilities

### New Capabilities

- `capture-intake`: the extractor decodes the typed capture command and publishes the document fact with the contract wrapper.
- `blob-sourced-extraction`: a blob capture of a Telegram-owned PDF is verified, stored, extracted and reported without a fetch.
- `bus-identity`: the extractor and the browser worker run under narrow NATS identities and never create topology in production.

### Modified Capabilities

(none; the existing event-pipeline requirements keep their wording and the new capabilities carry the changed behaviour)

## Impact

- Code: `crates/eventing` (decode, completion payload, blob run intake, consumer and render durable verification), `crates/persistence` (`QueuedRun`, schema checks), `crates/core` (config), `crates/test-support` (typed command helper), `services/extractor` (pipeline blob branch, boot), `services/browser-worker` (settings, nkey, durable and KV by get), `schema.sql`, `deploy/`, `compose.yaml`, `fuzz/` pins.
- Contracts: pins every `ratatoskr-*` dependency to ratatoskr-contracts `ad16855c4e7f3d52cd118274faa3b8f3ab4da576`. Cross-repository behaviour is defined in XR-021 CONTRACTS.md sections S01, S02, S03, S04, S05, S09 and S11; this change cites them and restates none of them.
- Docs: `README.md`, `docs/INTERFACES.md`, `DEVELOPMENT.md`, `deploy/README.md`.
