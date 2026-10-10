## 1. Contracts pin

- [x] 1.1 Move every `ratatoskr-*` contracts dependency in `Cargo.toml` and `fuzz/Cargo.toml` to ratatoskr-contracts `ad16855c4e7f3d52cd118274faa3b8f3ab4da576` and refresh both lock files. Why no failing test first: dependency pin; the existing suite staying green is the gate.

## 2. Typed capture command

- [x] 2.1 Add failing tests in `crates/eventing/tests/decode.rs`: `decodes_the_typed_content_capture_command`, `blob_payload_decodes_to_a_blob_source`, `payload_with_both_url_and_blob_is_rejected`, `an_aggregate_id_that_disagrees_with_the_payload_operation_is_rejected`, `url_payload_still_decodes_to_a_url_source`, using a typed-command helper in `crates/test-support`. Expected failure today: the flat decoder rejects the envelope (missing `requested_at`).
- [x] 2.2 Rewrite `decode_capture` over `CommandEnvelope` and `ContentCaptureRequested`, replace `CaptureCommand.url` with `source: CaptureSource`, delete `CaptureCommandWire`, `CapturePayload` and `COMMAND_PRODUCER`, and migrate every capture-command literal in the test suites to the helper. Verify: 2.1 green and the existing suites green.

## 3. Document fact wrapper

- [x] 3.1 Add failing test `crates/eventing/tests/completion.rs::document_event_payload_is_the_contract_type`. Expected failure today: the payload is the flat Document.
- [x] 3.2 Build `ContentDocumentExtracted` in `enqueue_completion_events`; update the assertions in `completion.rs` and `outbox.rs`. Verify: 3.1 green.

## 4. Blob source intake

- [x] 4.1 Add failing tests `crates/eventing/tests/command.rs::a_blob_capture_queues_a_blob_run`, `a_second_identical_blob_by_the_same_owner_reuses_the_source` and `crates/persistence/tests/schema.rs::a_blob_row_without_a_digest_violates_the_schema_check`. Expected failure today: a blob command is refused or the columns do not exist (the intake and schema gap, stated reason).
- [x] 4.2 Edit `schema.sql` in place with the blob source columns and checks, branch `queue_run` on `CaptureCommand.source`, and extend `claim_queued_run` and `QueuedRun` with `blob`. Verify: 4.1 green.

## 5. Peer blob root configuration

- [x] 5.1 Add failing test `crates/core/tests/config.rs::telegram_root_defaults_to_the_durable_layout_and_must_be_absolute`. Expected failure today: unknown key under `deny_unknown_fields`.
- [x] 5.2 Add `blobs.telegram_root` with absolute-path validation, the `extractor.conf.example` line, the unit's `SupplementaryGroups` and `ReadOnlyPaths`, and the deployment docs. Verify: 5.1 green.

## 6. Blob runs through the PDF path

- [x] 6.1 Add failing tests in `services/extractor/tests/blob_pdf_pipeline.rs`: `blob_pdf_run_completes_end_to_end`, `blob_run_with_missing_peer_file_fails_blob_missing`, `blob_run_with_tampered_peer_bytes_fails_blob_mismatch`, `blob_run_with_foreign_owner_fails_blob_owner`, `blob_run_with_image_media_type_fails_unsupported_media`, `blob_pdf_without_text_layer_records_pdf_no_text_layer`. Expected failure today: `process_run` treats the urn as an HTTP address.
- [x] 6.2 Branch `process_run` on the blob, verify via the peer store, copy into the extractor store, share `finish_pdf` with URL runs, and add `complete_blob_document` and `reject_blob_quality` over private inners. Verify: 6.1 green and the URL PDF tests unchanged.

## 7. Provisioning switch

- [x] 7.1 Add failing tests in `crates/core/tests/config.rs`: `provision_topology_defaults_off`, `provision_topology_true_with_nkey_seed_is_refused`, `durable_name_other_than_ratatoskr_extractor_capture_is_refused_when_not_provisioning`, `durable_name_is_free_when_provisioning`. Expected failure today: unknown key.
- [x] 7.2 Add `bus.provision_topology` and its validation; set it in the tests that spawn the binary against the unauthenticated broker; document it. Verify: 7.1 green.

## 8. Narrow extractor identity

- [x] 8.1 Add failing tests `crates/eventing/tests/authorized_bus.rs` (authorization-enabled broker matrix) and `crates/eventing/tests/identity_fragment.rs::fragment_has_no_broad_grants`. Expected failure today: `run_command_consumer` creates the stream and consumer and `request_render` calls `get_stream`; the fragment still grants `evt.>`.
- [x] 8.2 Replace the fragment with the EXTRACTOR stanza at `deploy/nats/identity.conf`, add `identity-browser-worker.conf`, make the command consumer and the render await verify and get by provisioning mode, make `main.rs` provision only behind the flag, and update `deploy/README.md`. Verify: 8.1 green.

## 9. Browser worker identity

- [x] 9.1 Add failing tests `services/browser-worker/tests/authorized_bus.rs` and the settings tests for `BROWSER_NKEY_SEED_PATH` and `BROWSER_PROVISION_TOPOLOGY`. Expected failure today: no credential, `ensure_render_stream` creates topology, the durable is get-or-create.
- [x] 9.2 Add `nkey_seed_path` and `provision_topology` to `WorkerSettings`, connect with the nkey, fetch the durable and KV by get in production, keep creation behind the flag, update existing tests and `compose.yaml`. Verify: 9.1 green.

## 10. Operator port

- [x] 10.1 Add failing test `crates/core/tests/config.rs::admin_default_is_the_allocated_operator_port`. Expected failure today: default is 9467.
- [x] 10.2 Move the default, the example and the documentation to 9088. Verify: 10.1 green.

## 11. Documentation and gate

- [x] 11.1 Update `README.md`, `docs/INTERFACES.md`, `DEVELOPMENT.md` to the implemented state and run the full gate including `openspec validate --all --strict`. Why no failing test first: documentation and verification over delivered work.
