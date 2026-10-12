## Why

Round 5 of changeset XR-021 re-verified at `b44ba81` that `queue_run` binds a freshly minted `DocumentId::new_v7()` into `extractor.extraction_runs.document_id` for every run, although the source row is content-addressed and reused. The same PDF captured twice (plain, then forwarded, or by two operations) therefore yields two `evt.content.document.extracted.v1` events with an equal `Document.content_digest` and different `document_id`, and Knowledge registers two documents. The behaviour is specified fleet-wide by XR-021 CONTRACTS.md R5-02; this change implements the extractor half (rules 1 to 3 and the extractor half of rule 5). Knowledge's half (rule 4) is its own change.

## What Changes

- **BREAKING (API)**: `extractor_document_ir::stable_document_id(owner, content_digest)` derives a document identity as the first 16 bytes of SHA-256 over `"ratatoskr.document.v1\0" || owner uuid bytes || content digest hex bytes`, with version nibble 8 and the RFC 4122 variant bits. `assemble_document` takes `owner: UserId` instead of `document_id: DocumentId` and derives the identity itself. `HtmlDocumentInput`, the PDF, YouTube and provider inputs carry `owner: UserId` instead of `document_id`. No parser can mint an identity.
- **BREAKING (schema, no migration)**: `extractor.extraction_runs.document_id` and its unique constraint are deleted from `schema.sql`. `QueuedRun.document_id` is replaced by `QueuedRun.owner`; the claim query reads `owner_id` and no longer reads a document id; `complete_*` match `run_id` and `status = 'running'` only. Development databases are recreated from `schema.sql`.
- Two operations over the same bytes for the same owner each complete, each emit their own `evt.content.document.extracted.v1` and their own operation report, and both name the one `document:<id>`.
- `README.md` and `docs/INTERFACES.md` state the identity rule.

## Capabilities

### New Capabilities

- `stable-document-identity`: a document's identity is a function of its owner and its content digest.

### Modified Capabilities

(none; the existing `document-ir` spec does not state the identity)

## Impact

- Code: `crates/document-ir`, `crates/pdf`, `crates/youtube`, `crates/providers`, `crates/eventing` (capture, completion, `QueuedRun`), `services/extractor` pipelines, `fuzz/`, `tools/corpus`, tests, `schema.sql`.
- Contracts: no change; `ContentDocumentExtracted` is unchanged and the contracts accept any RFC 9562 UUID as a `DocumentId`. Stays on ratatoskr-contracts `d98967b7b7ee80442c93ef4657140068955c8f61`. Cross-repository behaviour is defined in XR-021 CONTRACTS.md R5-02; this change cites it and restates only what the extractor must do.
- Out of scope: Knowledge source registration, raw blob dedup, document revisions for a URL whose text changed, the `max_event_bytes` oversize behaviour, image OCR.
