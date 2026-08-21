# Extractor interfaces

## Inbound

`cmd.content.capture.requested.v1`, using Platform's typed command envelope with owner, operation,
correlation, idempotency key, and public HTTP(S) URL.

## Outbound

`evt.content.document.extracted.v1` carries the shared Document payload.
`evt.platform.operation.reported.v1` carries queued, succeeded, or failed operation facts and
extractor-owned `BlobRef` values. No local filesystem path crosses the boundary.

## Internal boundaries

- `Fetcher`: one safe HTTP transaction sequence with limits and conditional cache.
- `SourceAdapter`: provider-native or format-specific conversion.
- `ArticleExtractor`: candidate from one parsed document.
- `QualityEvaluator`: deterministic score/reasons/threshold.
- `BrowserRenderer`: isolated final DOM/network evidence, not interpretation.
- `BlobStore`: content-addressed put/get/verify.

## Rules

Commands and events are idempotent and versioned. Errors distinguish policy, invalid input, unavailable source, resource limit, parser, browser, and transient dependency failures. Raw URLs and content are not logged. Browser requests cannot silently inherit provider credentials.

The command inbox, owned state, and initial report commit together. Successful fetch/Document IR
facts and both terminal outbox rows commit together. The publisher marks a row delivered only after
JetStream acknowledges it.
