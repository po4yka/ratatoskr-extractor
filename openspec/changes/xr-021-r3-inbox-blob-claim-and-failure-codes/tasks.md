## 1. Inbox prefixes (R3-01)

- [x] 1.1 Add failing tests `crates/eventing/tests/authorized_bus.rs::the_publisher_completes_jetstream_requests_under_its_own_inbox_prefix`, `services/browser-worker/tests/authorized_bus.rs::the_worker_reads_its_kv_bucket_under_its_own_inbox_prefix`, and update both `identity_fragment.rs` tests to the new subscribe grant. Expected failure today: the fetch and the bucket read time out because the connections reply on the default `_INBOX.` prefix, and the fragments still carry `_INBOX.>`.
- [x] 1.2 Add `INBOX_PREFIX` and apply it inside `NatsPublisher::connect_with_options` and `browser_worker::connect`; edit both `deploy/nats/identity*.conf` stanzas to the narrow grant. Verify: 1.1 green and the existing authorized-broker tests green.

## 2. The blob claim belongs to its run (R3-14)

- [x] 2.1 Add failing tests in `services/extractor/tests/blob_pdf_pipeline.rs`: `a_corrected_recapture_succeeds_after_a_wrong_length_claim`, `a_recapture_with_a_wrong_media_type_fails_unsupported_media_even_for_a_known_digest`, `a_recapture_with_a_wrong_length_fails_blob_mismatch_even_for_a_known_digest`, `two_runs_of_one_digest_verify_against_their_own_claims`. Expected failure today: the claim is read from the first source row of the digest.
- [x] 2.2 Move the three claim columns from `extractor.sources` to `extractor.extraction_runs` in `schema.sql`, write the claim in `queue_run` and read it in `claim_queued_run`. Verify: 2.1 green and the existing suites green.

## 3. Public failure codes (R3-14)

- [ ] 3.1 Add failing tests `crates/eventing/tests/completion.rs::each_failure_class_reports_its_public_code` and the end-to-end assertion in `services/extractor/tests/blob_pdf_pipeline.rs::blob_pdf_without_text_layer_records_pdf_no_text_layer`. Expected failure today: every failed report carries `content.extraction.failed`.
- [ ] 3.2 Add `public_failure(class)` beside `enqueue_failed_report`, make the latter take the class, and document the five codes in `docs/INTERFACES.md`. Verify: 3.1 green.

## 4. Browser worker operator example (R2-10 rule 2)

- [ ] 4.1 Add the failing test `services/browser-worker/tests/deployment_profile.rs::the_operator_example_loads_through_the_worker_settings`. Expected failure today: the example file does not exist.
- [ ] 4.2 Add `deploy/systemd/browser-worker.conf.example` and the "Browser worker" paragraph of `deploy/README.md`. Verify: 4.1 green.

## 5. Documentation (R3-19)

- [ ] 5.1 Rewrite the README workspace paragraph from the workspace as it is and name the inbox prefix in the README bus identity paragraph. Why no failing test first: documentation over delivered work.

## 6. Gate

- [ ] 6.1 Run the full gate including `cargo deny --locked check` and `openspec validate --all --strict`. Why no failing test first: verification over delivered work.
