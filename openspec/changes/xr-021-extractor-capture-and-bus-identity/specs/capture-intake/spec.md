# capture-intake Delta

## ADDED Requirements

### Requirement: Capture commands are typed command envelopes

Extractor SHALL decode `cmd.content.capture.requested.v1` as a contracts `CommandEnvelope` whose payload is `ContentCaptureRequested`, with exactly one of `url` or `blob`. It SHALL refuse an envelope whose producer is not `ratatoskr-platform` or `ratatoskr-x`, whose `tenant_id` is absent, or whose `aggregate_id` is not `operation:<payload.operation_id>`. No decoder for the former flat command document SHALL exist.

#### Scenario: a typed URL command is decoded

- **WHEN** the contracts fixture `content.capture.requested.v1/valid/url.json` is wrapped in an envelope and decoded
- **THEN** the command carries the payload operation id, the envelope tenant, and a URL source

#### Scenario: a typed blob command is decoded

- **WHEN** a payload names a sha256 blob
- **THEN** the command carries a blob source

#### Scenario: an ambiguous or inconsistent command is refused

- **WHEN** a payload names both a url and a blob, or the aggregate names a different operation
- **THEN** decoding fails with a typed error and no inbox row is written

### Requirement: The document fact uses the contract wrapper

A completed extraction SHALL enqueue `evt.content.document.extracted.v1` whose payload is `ContentDocumentExtracted`, with `document_blob` equal to the Document IR blob committed with the run, `aggregate_id` `document:<document_id>`, the capture's tenant, correlation, and causation `command:<command_id>`.

#### Scenario: the outbox row satisfies the contract type

- **WHEN** a run completes
- **THEN** the outbox payload parses with `EventEnvelope::payload_as::<ContentDocumentExtracted>()` and `document_blob` equals the committed IR blob

### Requirement: The operator listener default is the allocated port

The operator listener SHALL default to `127.0.0.1:9088`, and the shipped configuration example SHALL bind the same address.

#### Scenario: the default is the allocation

- **WHEN** the default configuration is built
- **THEN** `admin.bind` is `127.0.0.1:9088`
