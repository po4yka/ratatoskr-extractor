# pdf-extraction Specification

## Purpose
Defines bounded direct text extraction from verified PDF bytes into deterministic shared Document
IR, including explicit failure modes for encrypted, malformed, and oversized inputs and an explicit
degraded outcome for PDFs without a text layer.

## Requirements

### Requirement: PDF responses take the direct extraction path

Extractor SHALL route a response whose effective media type is `application/pdf` to direct PDF text
extraction using the already-verified raw artifact, SHALL keep those bytes as the extractor-owned
`raw_source` artifact referenced by BlobRef, and SHALL publish the same completion events as the
HTML path when extraction succeeds. The URL SHALL NOT be fetched a second time for extraction.

#### Scenario: a PDF URL completes like an article

- **WHEN** a queued run fetches bytes that start with `%PDF-` or declare `application/pdf` and the
  document carries extractable text
- **THEN** the run succeeds with one `direct_pdf` candidate, a `document_ir` artifact, a raw-source
  artifact whose media type is `application/pdf`, and outbox rows for
  `evt.content.document.extracted.v1` plus one succeeded operation report

### Requirement: Extraction is bounded and deterministic

Direct PDF extraction SHALL enforce finite budgets on source bytes, page count, and extracted text
size before publishing, SHALL preserve page order in block order where pages contribute text, and
SHALL produce identical blocks, provenance, and content digest for identical input bytes. Parse work
SHALL NOT block the async runtime and SHALL NOT execute embedded active content.

#### Scenario: identical input produces identical output

- **WHEN** the same fixture PDF is extracted twice within one process
- **THEN** both results have equal ordered blocks, equal content digest, and no second network
  request exists

#### Scenario: a budget is exceeded

- **WHEN** a PDF exceeds the configured input-byte, page-count, or extracted-text budget
- **THEN** extraction fails with a typed resource-limit error before any Document IR or completion
  event is produced

### Requirement: Encrypted PDFs fail explicitly

When a PDF requires a password that was not supplied, Extractor SHALL terminate the run with a
typed encryption failure class and SHALL NOT panic, guess credentials, or strip encryption by
reprocessing. A PDF encrypted with an empty user password MAY be extracted after decryption.

#### Scenario: a password-required PDF fails without crashing

- **WHEN** the fixture PDF encrypted with a non-empty user password is processed
- **THEN** the run terminates with the encryption failure class, the failed operation report is
  published, and no document event exists

### Requirement: Pathological files fail as typed errors

Malformed PDF structure, unsupported features, and parser-internal panics triggered by hostile
input SHALL surface as typed parse failures with a stable failure class. A parser panic SHALL be
contained at the extraction boundary and SHALL never abort the worker process.

#### Scenario: a corrupt PDF does not kill the worker

- **WHEN** truncated or structurally invalid PDF bytes are parsed
- **THEN** the run terminates with the parse failure class and the worker continues claiming later
  runs

### Requirement: PDFs without a text layer degrade explicitly

A PDF whose pages yield no usable text (a scanned document) SHALL terminate the run without a
document event, SHALL persist the rejected candidate evidence and raw-source artifact, and SHALL
record an explicit degraded failure class distinct from generic quality rejection. OCR remains out
of scope and no browser escalation SHALL follow from this outcome.

#### Scenario: an image-only PDF records its degraded evidence

- **WHEN** a PDF fixture with no text operators on any page is processed
- **THEN** the run fails with the degraded class, exactly zero-selected candidate rows are stored,
  and no `evt.content.document.extracted.v1` row exists
