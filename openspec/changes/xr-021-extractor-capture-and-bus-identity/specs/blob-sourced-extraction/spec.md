# blob-sourced-extraction Delta

## ADDED Requirements

### Requirement: A blob capture queues a PDF run without a fetch

A capture command naming a blob SHALL insert a `blob` source whose address is `urn:ratatoskr:blob:sha256:<hex>` with host `ratatoskr-telegram` and classification `blob`, and a queued run with parser version `pdf-v1`, without applying URL normalization or the network policy. The same owner re-sending the same blob SHALL reuse the source and queue a new run. A blob source row without a digest SHALL violate the schema.

#### Scenario: a blob run is queued

- **WHEN** a blob capture is consumed
- **THEN** one blob source and one queued `pdf-v1` run exist and the claimed run carries the blob reference

#### Scenario: a repeated blob reuses its source

- **WHEN** the same owner submits the same blob in two commands
- **THEN** one source row and two runs exist

### Requirement: Peer bytes are verified before use

Extractor SHALL read a blob run from the Telegram blob root only after verifying owner, sha256 and length, SHALL copy the verified bytes into its own store, and SHALL never write to the peer root. A missing file, an owner other than `ratatoskr-telegram`, a digest or length mismatch, an unreadable file, or a media type other than `application/pdf` SHALL fail the run, non-retryable, as `blob_missing`, `blob_owner`, `blob_mismatch`, `blob_unreadable` or `unsupported_media`.

#### Scenario: a Telegram PDF is extracted end to end

- **WHEN** a valid text PDF is in the Telegram root and a blob command is processed
- **THEN** the run succeeds, the document address is the blob urn, provenance names an extractor-owned source blob with the same digest, `raw_source` and `document_ir` artifacts exist, no fetch row exists, and the outbox holds the document fact and a succeeded report

#### Scenario: bad peer bytes fail the run

- **WHEN** the peer file is missing, tampered, foreign-owned, or the media type is an image
- **THEN** the run fails with the matching class and no document fact is enqueued

#### Scenario: a PDF without a text layer degrades explicitly

- **WHEN** the blob PDF has no text layer
- **THEN** the run fails as `pdf_no_text_layer`

### Requirement: The peer blob root is configured and mounted read-only

`blobs.telegram_root` SHALL default to `/mnt/nvme/ratatoskr/blobs/ratatoskr-telegram` and SHALL be absolute; the systemd unit SHALL list it under `ReadOnlyPaths` and add the `ratatoskr-telegram-blobs` supplementary group.

#### Scenario: a relative root is refused

- **WHEN** `RATATOSKR__BLOBS__TELEGRAM_ROOT` is relative
- **THEN** configuration validation names `blobs.telegram_root`
