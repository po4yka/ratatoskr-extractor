## 1. Fixture provenance and crate scaffold

- [ ] 1.1 Add `crates/pdf/tests/fixtures/generate.py` (stdlib only) emitting deterministic synthetic fixtures — `text-two-pages.pdf`, `multi-column.pdf`, `encrypted-user-password.pdf` (RC4, non-empty user password), `encrypted-blank-password.pdf`, `no-text-layer.pdf`, `oversized-padded.pdf`, `corrupt-truncated.pdf` — commit their bytes, and verify a regeneration run reproduces identical SHA-256 digests. No failing test: generated binary fixtures with first-party provenance; verification is the digest comparison.
- [ ] 1.2 Scaffold workspace crate `crates/pdf` (`ratatoskr-extractor-pdf`, lib `extractor_pdf`) wired into `Cargo.toml`, depending only on contracts, identifiers, document-ir, thiserror, sha2, serde_json, and `pdf-extract =0.12.0`; update `Cargo.lock`. Verify `cargo build -p ratatoskr-extractor-pdf --locked` compiles and `cargo deny --locked check` stays green. No failing test: configuration and dependency admission.

## 2. Shared plain-text evaluator

- [ ] 2.1 Add failing test `crates/document-ir/tests/plain_text.rs::plain_text_candidate_reuses_shared_thresholds`: stub `evaluate_plain_text` returning a never-accepted decision, assert exact metrics components, score, accepted=true, reasons, and evaluator version for a 300-character single paragraph with matching title. Run it and confirm the assertion fails against the stub.
- [ ] 2.2 Implement `evaluate_plain_text(strategy, blocks, title)` in `crates/document-ir/src/lib.rs` reusing `evaluate` with zero link/boilerplate exclusions, make 2.1 pass, run document-ir format and Clippy gates, and commit this TDD pair.

## 3. Direct PDF extraction core

- [ ] 3.1 Add failing test `crates/pdf/tests/extract.rs::text_pdf_yields_page_ordered_paragraphs`: call the stubbed `from_pdf` entry point over `text-two-pages.pdf` with generous limits; assert two Paragraph blocks in page order containing the known per-page sentences, title from the Info dictionary, provenance naming `direct_pdf` with the source BlobRef on every block, one selected accepted candidate, and equal content digest across two invocations. Run it and confirm it fails against a stub returning an unimplemented error variant.
- [ ] 3.2 Implement `from_pdf` in `crates/pdf/src/lib.rs`: input-byte check, one `load_mem` parse, blank-password decryption when encrypted, ascending page walk via `output_doc_page` into fresh `PlainTextOutput<String>` per page, whitespace normalization to one Paragraph per non-empty page, shared-evaluator candidate selection, canonical digest and provenance; make 3.1 pass, run pdf-crate format and Clippy gates, and commit this TDD pair.
- [ ] 3.3 Add failing test `crates/pdf/tests/extract.rs::multi_column_text_is_preserved_in_one_pass` asserting both fixture columns appear in the extracted blocks with no second parse; run it and confirm the current implementation misses the second column or fails, then fix extraction only if genuinely broken and record observed ordering; make it pass, run gates, and commit this pair.
- [ ] 3.4 Add failing test `crates/pdf/tests/determinism.rs::repeated_extraction_is_byte_stable`: extract every text-bearing fixture twice and assert equal `PdfExtraction` values plus one golden digest constant blessed from the text fixture. Run it and confirm the golden constant fails before blessing; bless, make it pass, run gates, and commit this pair.

## 4. Typed failure modes

- [ ] 4.1 Add failing tests `crates/pdf/tests/failures.rs::password_required_pdf_is_typed_encrypted`, `::blank_password_encrypted_pdf_extracts`, `::oversized_input_is_resource_limit`, `::page_and_text_budgets_stop_extraction`, `::no_text_layer_pdf_degrades_with_candidates`, and `::corrupt_pdf_is_malformed_not_panic`: assert typed `PdfError` variants (and candidate evidence for degradation) across the encrypted, oversized, no-text-layer, and corrupt fixtures with tight budgets, plus worker survival semantics for panics. Run them and confirm each fails against the current success-or-crash behavior.
- [ ] 4.2 Implement the typed error mapping: password-required → `Encrypted`, budget checks before and between page extractions → `ResourceLimit`, zero accepted text → `NoTextLayer { candidates }`, parse errors and catch-unwind containment → `Malformed`; make 4.1 pass, run pdf-crate and document-ir gates, and commit this TDD pair.

## 5. Eventing generalization

- [ ] 5.1 Add failing real-PostgreSQL test `services/extractor/tests/command_pipeline.rs::single_candidate_completion_commits_like_html`: drive consume→claim→complete_document with one accepted `direct_pdf` candidate and assert candidate/artifact/outbox facts match the three-candidate shape. Run it and confirm `ConsumeError::InvalidRunState` today.
- [ ] 5.2 Relax `validate_candidates` to any non-empty set with the expected selected count, make 5.1 pass alongside existing eventing tests, run eventing gates, and commit this TDD pair.
- [ ] 5.3 Add failing real-PostgreSQL tests `quality_rejection_records_explicit_class` (reject_quality with class `pdf_no_text_layer` persists that `last_error_class`) and `pdf_classified_run_records_pdf_parser_version` (consume a `.pdf` URL, read `parser_version`). Run them and confirm the hard-coded class and `html-v1` fail the assertions.
- [ ] 5.4 Add the failure-class parameter to `reject_quality` (HTML callers pass `quality`) and derive `parser_version` from `SourceRoute` at intake (`pdf-v1` / `html-v1`), update all call sites, make 5.3 pass, run eventing and service gates, and commit this TDD pair.

## 6. Service routing and budgets

- [ ] 6.1 Add failing config test `crates/core/tests/config.rs::pdf_defaults_are_bounded` asserting new `PdfConfig` defaults and env overrides parse; run it and confirm the section is absent.
- [ ] 6.2 Add `PdfConfig { max_input_bytes, max_pages, max_text_bytes }` with defaults 50 MiB / 1000 / 8 MiB to `ExtractorConfig`, make 6.1 pass, run core gates, and commit this TDD pair.
- [ ] 6.3 Move `process_run` into `services/extractor/src/lib.rs` unchanged and add failing integration test `services/extractor/tests/command_pipeline.rs::pdf_media_type_takes_direct_path_end_to_end`: queue a run against the scripted server serving `text-two-pages.pdf`, claim, call `process_run` with a SafeFetcher configured for the scripted port, assert succeeded status, `direct_pdf` candidate, raw_source artifact media type `application/pdf`, document_ir artifact, both outbox subjects. Run it and confirm `unsupported_media` failure today.
- [ ] 6.4 Route `application/pdf` through the PDF path in `process_run` (typed error classes per design, spawn_blocking, degraded rejection), keep HTML and unsupported media behavior byte-for-byte, make 6.3 pass, add the scripted-server PDF response helper if needed, run service/test-support gates, and commit this TDD pair.
- [ ] 6.5 Add failing integration test `pdf_failure_classes_reach_terminal_state` covering encrypted (class `pdf_encrypted`) and corrupt (class `parse`) fixtures end-to-end through `process_run`; run it and confirm those classes are absent, then make it pass and commit this pair.

## 7. Documentation and completion

- [ ] 7.1 Update `DEVELOPMENT.md` status paragraph and README deferred-feature notes to state direct PDF extraction exists while OCR, provider adapters, and browser escalation remain deferred; tick the PDF half of plan item 7 in `docs/IMPLEMENTATION_PLAN.md` by splitting the item so provider adapters stay open. No test: documentation; verify statements against built behavior.
- [ ] 7.2 Run the exact `DEVELOPMENT.md` gate order including real PostgreSQL/JetStream tests, the file-size ratchet, `openspec validate add-direct-pdf-extraction --strict`, and inspect the final diff. Commit the change archive only after every gate is green, integrate into `main`, push, and verify the remote SHA.
