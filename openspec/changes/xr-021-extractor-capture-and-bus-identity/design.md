## Context

Cross-repository decisions live in XR-021 CONTRACTS.md (S01 subjects, S02 lifecycle rules, S03 identities, S04 durables, S05 ports, S09 document fact, S11 capture command and blob captures). This design records only the extractor-internal choices.

## Decisions

### D1. One decoder, no legacy shape

`decode_capture` parses `CommandEnvelope::from_json`, checks the subject, the command type and the producer (`ratatoskr-platform` or `ratatoskr-x`), requires `tenant_id`, calls `payload_as::<ContentCaptureRequested>` (whose deserializer already refuses both-or-neither of url and blob), and requires `aggregate_id == operation:<payload.operation_id>`. `issued_at` becomes `requested_at`; the idempotency key becomes its hex. The old wire struct is deleted and no fallback decoder exists. Test commands are produced by one helper in `crates/test-support` so every literal in the suite uses the contract shape.

### D2. The document fact is built from the contract type

`enqueue_completion_events` builds `ContentDocumentExtracted { document, document_blob: ir_blob, extensions }` and sets it with `EventEnvelope::set_payload`, so the event type and the body cannot disagree. The operation report is unchanged.

### D3. Blob sources are sources, not fetches

A blob capture inserts an `extractor.sources` row of kind `blob` with the urn `urn:ratatoskr:blob:sha256:<hex>` as every address column and `ratatoskr-telegram` as host, so source reuse by `(owner_id, normalized_url)` works unchanged: the same owner re-sending the same PDF reuses the row and queues a new run. No `extractor.fetches` row is written; `complete_document` and `reject_quality` become private inners taking `Option<&CompletedFetch>` and an explicit raw `BlobRef`, with public wrappers for blob runs. A table check ties the blob columns to `source_kind = 'blob'`.

### D4. Peer reads are verified copies

`process_run` branches first on `run.blob`. It opens the Telegram store read-only with `BlobStore::new(root).with_owner("ratatoskr-telegram")`, verifies owner, digest and length (re-hash), reads the verified file once, and stores those same bytes in the extractor store, which re-hashes them and produces an extractor-owned raw `BlobRef`; the copy must carry the digest and length of the reference. The PDF parse then runs on that one buffer, so the document describes exactly the bytes the reference names even if the peer file changes after verification. A reference longer than `pdf.max_input_bytes` fails as `parse` before anything is read into memory. The peer root is never written. The PDF finishing code is split into `services/extractor/src/pdf_run.rs` so URL and blob runs share one `finish_pdf` and no file passes the 850-line cap.

### D5. Consumers verify, only provisioning mode creates

`provision_topology` (default false, refused together with an nkey seed) selects between `ensure_*`/`get_or_create_*` (unauthenticated local brokers and tests) and `get_consumer_from_stream` plus verification of filter, ack policy, ack wait and max_deliver against the S04 constants. A mismatch or a missing durable keeps readiness false with an error naming what is missing and saying to start the edge first.

### D6. Identity fragments are copies, not sources

`deploy/nats/identity.conf` and `identity-browser-worker.conf` carry the exact stanza text of Platform's `ratatoskr.conf`; tests start an authorization-enabled `nats-server` from them with generated keys substituted, so the first real proof of the KV subject list for the browser worker is the matrix in `services/browser-worker/tests/authorized_bus.rs`.

### D7. Render results are polled, not streamed

With the fixed `ratatoskr_extractor_render_awaits` durable, a long-lived `messages()` stream per request would leave a server-side pull request pending after the call returns, and that stale request could swallow the next request's completion because the durable is shared. `request_render` therefore issues short non-waiting `fetch` pulls (every 100 ms until the render budget elapses), so no pull request outlives the call. A development process still creates its own consumer behind `provision_topology`.

### D8. The authorized-broker tests start their own broker

`crates/test-support/src/broker.rs` starts a throwaway `nats-server` with an administrator (user and password, all permissions) plus the nkey identities under test, built from the shipped `deploy/nats/identity*.conf` with generated keys substituted. It needs the `nats-server` executable (`NATS_SERVER_BIN`), which CI copies out of the broker image; the tests fail rather than skip when it is missing. The fragments are written in the layout the S03 stanzas use, and `identity_fragment.rs` pins their normalised text to the S03 lists.

## Risks

- Interleaved old and new capture producers: old commands are dropped with a Term-class decode error. Deploy Platform, X and the extractor in one window.
- A missing `ratatoskr-telegram-blobs` group on the host yields `blob_unreadable` runs; host state no repository can apply.
- The identity fragments equal the S03 stanza text only as far as Platform formats its stanza the same way (trailing commas inside the arrays); the workspace equality check ignores comments, whitespace and the nkey token, not formatting differences of that kind.
- The shared `evt.platform.operation.reported.v1` subject remains trust-by-producer-field (S03 residual risk).
