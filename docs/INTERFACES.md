# Extractor interfaces

## Inbound

`cmd.content.capture.requested.v1`, a `CommandEnvelope` from `ratatoskr-event-envelope` whose
producer is `ratatoskr-platform` or `ratatoskr-x`, whose `tenant_id` is required (`user:<uuid>`) and
whose `aggregate_id` is `operation:<operation_id>`. The extractor refuses an envelope whose aggregate
disagrees with the payload operation. The payload is `ContentCaptureRequested` from
`ratatoskr-document-contracts`: `operation_id`, `idempotency_key` (the SHA-256 of the caller's
idempotency string) and exactly one source, either `url` (an `http` or `https` address) or `blob` (a
`BlobRef` the Telegram service owns). A command that is not decodable, names the wrong producer or
omits the tenant is acknowledged and dropped (the poison disposition), never redelivered.

A blob capture reads the bytes from the Telegram blob root (`blobs.telegram_root`), verifies owner,
SHA-256 and length against the reference, and copies them into the extractor's own store; only
`application/pdf` is extracted. The source address is `urn:ratatoskr:blob:sha256:<hex>`, the host is
`ratatoskr-telegram`, no fetch happens, and the run records no `extractor.fetches` row. Failure
classes for a blob run are `blob_owner`, `blob_missing`, `blob_mismatch`, `blob_unreadable`,
`unsupported_media`, `pdf_encrypted`, `pdf_no_text_layer` and `parse`.

## Outbound

`evt.content.document.extracted.v1` carries `ContentDocumentExtracted`: the `document` inline, and
`document_blob`, the extractor-owned Document IR blob (the same `BlobRef` the operation report
carries). Its envelope names `aggregate_id` `document:<document_id>`, the capture command's
correlation, `command:<command_id>` as causation, and the owning tenant.
`evt.platform.operation.reported.v1` carries queued, succeeded, or failed operation facts and
extractor-owned `BlobRef` values. No local filesystem path crosses the boundary.

`cmd.content.render.requested.v1` is the extractor-internal command to the browser worker; its
results return on `evt.content.render.completed.v1` and `evt.content.render.failed.v1`. These three
subjects are not registered contracts.

## Bus identity

The extractor holds one nkey identity that may consume `ratatoskr_extractor_capture`, read
`ratatoskr_extractor_render_awaits`, and publish the two facts above plus the render command. It
creates no stream, consumer or bucket; Edge provisions them (`deploy/README.md`).

## Internal boundaries

- `Fetcher`: one safe HTTP transaction sequence with limits and conditional cache.
- `SourceAdapter`: provider-native or format-specific conversion (Hacker News, Reddit,
  YouTube transcripts, direct PDF).
- `ArticleExtractor`: candidate from one parsed document.
- `QualityEvaluator`: deterministic score/reasons/threshold.
- `BrowserRenderer`: isolated final DOM/network evidence, not interpretation.
- `BlobStore`: content-addressed put/get/verify.

## Rules

Commands and events are idempotent and versioned. Errors distinguish policy, invalid input, unavailable source, resource limit, parser, browser, and transient dependency failures. Raw URLs and content are not logged. Browser requests cannot silently inherit provider credentials.

The command inbox, owned state, and initial report commit together. Successful fetch/Document IR
facts and both terminal outbox rows commit together. The publisher marks a row delivered only after
JetStream acknowledges it.
