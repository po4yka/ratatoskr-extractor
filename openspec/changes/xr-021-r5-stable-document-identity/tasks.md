## 1. The derivation (R5-02 rules 1 and 2)

- [x] 1.1 Add the failing tests `crates/document-ir/tests/stable_identity.rs`: `stable_document_id_is_deterministic`, `stable_document_id_differs_across_owners`, `stable_document_id_differs_across_digests`, `stable_document_id_is_version_8_with_the_rfc_4122_variant`, `stable_document_id_matches_the_golden_value`, `assemble_document_derives_the_identity_from_the_owner`. A signature-only stub returns the nil id and `assemble_document` already takes `owner`. Expected failure today: the assertions on determinism-by-owner, version nibble 8 and the golden literal, because the stub returns the nil id.
- [x] 1.2 Implement `stable_document_id`, make `assemble_document` derive the identity from its owner and digest, replace `document_id` by `owner` in `HtmlDocumentInput` and the PDF, YouTube and provider inputs, and update every call site (the six extraction paths, the fuzz targets, `tools/corpus`, the existing tests). Verify: 1.1 green and the existing suites green.

## 2. Runs no longer carry an identity (R5-02 rule 3)

- [x] 2.1 Add the failing tests in `crates/eventing/tests/completion.rs`: `two_operations_over_one_digest_share_one_document` (equal `document_id` and `content_digest`, two event ids, two operations, both reports name `document:<id>`) and `a_claimed_run_exposes_the_owner_and_no_document_id`. Expected failure today: the second run's `document_id` differs because the insert binds a minted id.
- [x] 2.2 Delete `extraction_runs.document_id` from `schema.sql`, replace `QueuedRun.document_id` by `QueuedRun.owner`, stop binding and matching `document_id` in `capture.rs` and `completion.rs`, and update every test insert. Verify: 2.1 green and the existing suites green.

## 3. Same bytes, two routes (R5-02 rule 5)

- [x] 3.1 Add the failing tests in `services/extractor/tests/pdf_pipeline.rs`: `the_same_pdf_by_blob_and_by_url_yields_one_document_id`, `a_pdf_with_other_text_yields_another_document_id`, `the_same_pdf_for_another_owner_yields_another_document_id`. Expected failure today: the route-specific minted ids differ. Recorded deviation: the tests were written after 1.2 and 2.2 landed, because the green state needed nothing else; `the_same_pdf_by_blob_and_by_url_yields_one_document_id` was shown to fail (`left == right` on the two `document_id` values) when `assemble_document` was temporarily changed to mint `DocumentId::new_v7()`, and the change was then reverted. The URL leg queues its run with the intake rows directly because intake refuses the scripted server's non-standard port (`PortDenied`).
- [x] 3.2 Make the tests pass; nothing beyond 1.2 and 2.2 was needed, and the PDF path adds no route-specific block, so the digest depends on the text blocks only. Verify: 3.1 green.

## 4. Documentation

- [x] 4.1 State the identity rule in `README.md` and `docs/INTERFACES.md`. Why no failing test first: documentation over delivered work.

## 5. Gate

- [x] 5.1 Run the full gate of `DEVELOPMENT.md`, `cargo deny --locked check`, `cargo check --locked` in `fuzz/` and `openspec validate --all --strict`. Why no failing test first: verification over delivered work.
