## Why

The extractor terminates every `application/pdf` response as `unsupported_media` after the bytes
were already fetched and stored, so the legacy monolith's "direct PDF" rung has no counterpart here.
Plan item 7 calls for direct PDF extraction before any provider adapters or browser escalation, and
Knowledge cannot ingest PDF sources until the ordinary path produces Document IR from them.

## What Changes

- Route `application/pdf` responses (declared or `%PDF-` sniffed) into a direct PDF extraction path
  instead of the `unsupported_media` terminal failure.
- Add a pure-Rust PDF text extraction crate behind a narrow parse-once entry point that produces
  text-block Document IR in page order, with the raw PDF bytes kept as the extractor-owned
  `raw_source` artifact through the existing BlobRef mechanism.
- Treat encrypted PDFs that require a password, malformed/pathological files, and budget
  overruns as typed failure modes, never panics; record scanned PDFs without a text layer as an
  explicit degraded terminal outcome with persisted candidate evidence.
- Generalize candidate persistence so a run may commit fewer than three candidates (one
  `direct_pdf` candidate today) and let terminal quality rejection carry an explicit failure class.
- Record a route-appropriate parser version (`pdf-v1`) at command intake for PDF-classified sources.
- Commit synthetic fixture PDFs with their generator script (text, multi-column, encrypted,
  oversized, no-text-layer) and mark plan item 7's PDF half complete.
- Keep OCR, browser escalation, provider adapters, and PDF form/annotation handling outside this
  change.

## Capabilities

### New Capabilities

- `pdf-extraction`: Direct bounded text extraction from verified PDF bytes into deterministic
  Document IR, including encryption refusal, resource budgets, degraded no-text-layer outcomes, and
  page-order preservation.

### Modified Capabilities

- `event-pipeline`: Candidate persistence accepts any non-empty candidate set rather than exactly
  three; quality rejection records an explicit failure class; queued runs record the parser version
  implied by the source classification.

## Impact

New crate `crates/pdf` depends on `pdf-extract 0.12` (MIT, pure Rust, vendored through cargo-deny).
`crates/document-ir` gains one public plain-text evaluator entry point. `crates/eventing`
generalizes candidate validation and rejection class; `services/extractor` routes PDF media types;
`crates/core` gains PDF parser budgets; fixtures land under `crates/pdf/tests/fixtures`. No shared
Document IR field, event subject, fetch count, or HTML-path behavior changes.
