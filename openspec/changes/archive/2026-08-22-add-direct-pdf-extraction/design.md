## Context

`process_run` fetches once into a BlobRef-addressed artifact and currently terminates any media type
other than `text/html` as `unsupported_media`; `application/pdf` responses therefore die after the
download. The HTML path parses once, evaluates three candidates with the shared deterministic
evaluator, and commits run/fetch/artifacts/candidates plus two outbox rows in one transaction.
Candidate validation in the eventing terminal path hard-codes exactly three candidates, and
`reject_quality` hard-codes the `quality` failure class. Intake records `html-v1` as the parser
version for every source.

## Goals / Non-Goals

Goals: one bounded parse of verified PDF bytes; page-ordered text blocks in Document IR; typed,
non-panicking failure modes; explicit degraded outcome for text-less PDFs; reuse of the shared
completion transaction, events, and quality thresholds.

Non-Goals: OCR or image handling; browser escalation from PDF outcomes; provider-specific PDF
adapters; form field, annotation, or digital-signature extraction; password configuration surfaces
(no credential store exists); changing any HTML-path behavior or shared contract shape.

## Decisions

### Dependency: `pdf-extract 0.12` (MIT) over pdfium-render, `pdf`, or a hand-rolled lopdf walker

Supply-chain evaluation against repository policy:

- **pdf-extract 0.12.0** — pure Rust, MIT, released June 2026 (actively maintained), ~4.1M total
  downloads. Direct dependencies are nine small, long-established pure-Rust crates (`lopdf 0.42`,
  `adobe-cmap-parser`, `cff-parser`, `type1-encoding-parser`, `postscript`, `encoding_rs`,
  `euclid`, `log`, `unicode-normalization`); no C toolchain, no system library, no network-enabled
  production code (its `ureq` dependency is dev-only). It handles the hard parts we must not
  hand-roll: ToUnicode/CMap decoding, CID and simple font encodings, TJ/Tj positioning heuristics.
- **pdfium-render** rejected — binds Google's C++ Pdfium via a dynamically loaded native library;
  a large native supply-chain artifact per target platform contradicts the vendored, deny-checked,
  pure-Rust dependency posture of this workspace.
- **`pdf` (pdf-rs)** rejected for this change — lower-level reader without a maintained plain-text
  layout output; adopting it means writing and maintaining our own content-stream interpreter and
  CMap handling, which is exactly the bug-prone surface we should not own.
- **Hand-rolled lopdf walker** rejected for the same reason, plus lopdf alone does not solve font
  encoding.

Risk acceptance: pdf-extract returns `Result` for structural errors but panics on numerous hostile
inputs (short `/MediaBox` arrays, operators with missing operands, unknown color spaces, malformed
CMaps). Accepted knowingly because the crate is isolated behind a new leaf crate, pinned to an
exact version like every workspace dependency, screened by `cargo deny` (advisories, licenses,
bans), and wrapped in a panic-containment boundary (below). A panic there is a typed failure, not a
process crash; if advisories appear, the boundary makes replacement a single-crate change. One
advisory is accepted with a documented ignore: RUSTSEC-2026-0192 marks `ttf-parser` unmaintained;
it reaches us only through pdf-extract → lopdf embedded-font parsing, no safe upgrade exists, and
the deny entry records the revisit trigger.

### Placement: new leaf crate `crates/pdf`

PDF parsing deps stay out of `document-ir`, which `eventing`/`persistence` depend on; cargo would
otherwise compile PDF crates into every dependent build. `crates/pdf` depends on the contracts,
`ratatoskr-identifiers`, `document-ir` (candidate types), and `pdf-extract`. The public surface is
one entry point: `from_pdf(PdfDocumentInput, PdfParseLimits) -> Result<PdfExtraction, PdfError>`.

### Parse shape: load once, walk pages explicitly

Use `lopdf::Document::load_mem` (re-exported through pdf-extract) once, then decrypt-if-needed, then
iterate `get_pages()` (a `BTreeMap<u32, ObjectId>` in ascending page-tree order) extracting each
page with pdf-extract's public `output_doc_page` into a fresh `PlainTextOutput<String>`. This avoids
`extract_text_from_mem_by_pages`, whose loop treats every per-page error as silent end-of-document;
we want typed failures instead. Page count is checked before extraction begins and the accumulated
text budget between pages, so oversized documents stop early.

Blocks: each non-empty page becomes one `Paragraph` block holding whitespace-normalized text.
PlainTextOutput signals line breaks but not paragraph gaps, so finer splitting would be invention;
page granularity preserves reading order deterministically. Title comes from the Info dictionary
`Title` via pdf-extract's `decode_text_string`. Language is not asserted by this path.

### Quality: shared evaluator, single candidate

Expose one format-neutral function from `document-ir`
(`evaluate_plain_text(strategy, blocks, title)`) that reuses the existing `quality_v1` components
and thresholds (text volume, paragraph distribution, non-link/non-boilerplate shares granted full
weight because extracted PDF text carries no link/boilerplate markup, title agreement). The PDF path
produces exactly one `direct_pdf` candidate; if it is not accepted, that decision list flows to the
rejection terminal so diagnostics persist. Duplicating threshold constants in `crates/pdf` was
rejected because threshold changes require golden-corpus evaluation and two copies would drift.

### Failure mapping

`PdfError::{ResourceLimit, Encrypted, Malformed, NoTextLayer, InvalidIdentity, Serialization}` map
to failure classes: resource limits → `parse` (bounded input is a parse property today);
password-required encryption → `pdf_encrypted`; parser errors and contained panics → `parse`;
no accepted candidate → explicit degraded class `pdf_no_text_layer` through the generalized quality
rejection terminal (persisting fetch, raw artifact, and the zero-scored candidate evidence).
Panics from pdf-extract are caught at the crate boundary with `catch_unwind` around the extraction
call (safe code; inputs are bytes, outputs freshly built strings) and become `Malformed`;
`spawn_blocking`'s `JoinError` remains the second containment layer in the service.

### Eventing generalization

`validate_candidates` requires a non-empty set with the expected selected count instead of exactly
three. `reject_quality` gains an explicit failure-class parameter (HTML keeps `quality`). Intake
sets `parser_version` from the source route: `pdf-v1` for `SourceRoute::Pdf`, `html-v1` otherwise.
No schema change: `candidates` rows already allow any strategy/version pair, and status/class
columns already accept these values.

### Budgets and runtime placement

New `PdfConfig { max_input_bytes, max_pages, max_text_bytes }` (defaults 50 MiB, 1,000 pages, 8 MiB)
enforced inside `from_pdf`; extraction runs in `tokio::task::spawn_blocking` like the HTML parse.
No wall-clock race is added: it matches the HTML path's budget model (size-bounded parse, fetch-level
timeouts) and an abandoned blocking thread would keep running anyway. Residual decompression
amplification inside lopdf's object loading is bounded by the wire/decoded fetch budgets and the
input-byte check; noted as risk below.

### Fixtures and their provenance

Synthetic fixtures generated by a committed Python script (`crates/pdf/tests/fixtures/generate.py`,
stdlib only) so bytes are reproducible and provenance is first-party: single-page text, two-column
layout, RC4-encrypted requiring a user password, blank-user-password encrypted, no-text-layer page,
and a padded oversized file exercised against a small configured budget rather than a huge blob in
git.

## Risks / Trade-offs

[Parser panics on hostile input] → catch_unwind boundary + spawn_blocking backstop + typed error;
fuzz-target follow-up stays open under plan item 9.
[Multipage memory amplification inside lopdf] → fetch budgets cap input; input-byte and page-count
checks stop early; text budget caps accumulation.
[One block per page loses intra-page paragraph structure] → deliberate v1 trade-off; block
refinement can land later without contract changes because blocks stay ordered Paragraphs.
[Multi-column reading order interleaves columns] → PlainTextOutput emits column breaks as newlines
within the same block; fixture asserts both columns present, not perfect ordering.
[Blank-password encrypted PDFs auto-decrypt] → intentional; content is readable, and the encrypted
provenance remains visible in the stored original bytes.
[Relaxed candidate validation weakens an invariant] → replaced by "non-empty, expected selected
count"; three-candidate HTML runs are still enforced upstream by the HTML extractor itself.

## Migration Plan

Single deployable, additive path. Before: PDF URLs fail terminally as `unsupported_media`. After:
they extract. No queued-run backfill is needed; failed PDF runs were final. Rollback is reverting the
deploy; no schema or contract state survives that requires reprocessing.

## Open Questions

None blocking; golden-corpus calibration across real-world publisher PDFs belongs to plan item 9.
