# Extractor interfaces

## Inbound

Typed extraction commands containing owner, operation, URL/blob reference, source hint, policy, correlation, idempotency, and schema version.

## Outbound

Extraction completed/failed events; immutable raw/rendered/IR blob references; safe diagnostics and operation progress; linked-source discovery events.

## Internal boundaries

- `Fetcher`: one safe HTTP transaction sequence with limits and conditional cache.
- `SourceAdapter`: provider-native or format-specific conversion.
- `ArticleExtractor`: candidate from one parsed document.
- `QualityEvaluator`: deterministic score/reasons/threshold.
- `BrowserRenderer`: isolated final DOM/network evidence, not interpretation.
- `BlobStore`: content-addressed put/get/verify.

## Rules

Commands and events are idempotent and versioned. Errors distinguish policy, invalid input, unavailable source, resource limit, parser, browser, and transient dependency failures. Raw URLs and content are not logged. Browser requests cannot silently inherit provider credentials.
