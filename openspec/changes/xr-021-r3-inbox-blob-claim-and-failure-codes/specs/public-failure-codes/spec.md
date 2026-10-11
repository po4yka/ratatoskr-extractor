# public-failure-codes Delta

## ADDED Requirements

### Requirement: A failed run reports a public code from a closed set

A failed run SHALL enqueue a failed operation report whose error code and safe message follow the run's failure class: `pdf_no_text_layer` gives `content.extraction.no_text_layer`, `pdf_encrypted` gives `content.extraction.encrypted`, `unsupported_media` gives `content.extraction.unsupported_media`, `blob_owner`, `blob_missing`, `blob_unreadable` and `blob_mismatch` give `content.extraction.source_unavailable`, and every other class gives `content.extraction.failed`. The class name SHALL NOT appear in the report.

#### Scenario: a PDF without a text layer

- **WHEN** a captured PDF has no text layer
- **THEN** the operation report carries `content.extraction.no_text_layer`
