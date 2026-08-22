# Durable extraction event pipeline

## Purpose

Defines durable command intake, extractor-owned execution state, and acknowledged event publication
that remain safe across redelivery, retry, and process restart.

## Requirements

### Requirement: Commands become durable owned work exactly once

Extractor SHALL consume `cmd.content.capture.requested.v1` at least once and SHALL insert its inbox
record, source, and queued extraction run in one transaction. The command identifier SHALL be the
deduplication key. The run SHALL receive its stable document identifier in that transaction so a
lease retry cannot change the document identity. Network or parser work SHALL NOT run while that
transaction is open.

#### Scenario: JetStream redelivers a command

- **WHEN** the same command identifier arrives more than once
- **THEN** Extractor records one inbox entry and executes one extraction run

#### Scenario: Extractor starts after a command was published

- **WHEN** a command is retained before the durable consumer is first created
- **THEN** Extractor consumes it instead of starting at the end of the stream

### Requirement: Owned facts and outgoing messages commit together

Extractor SHALL persist run, fetch, artifact, and candidate facts only under `extractor.*`. A
successful primitive extraction SHALL commit its run/result records and durable outbox rows for the
shared Document IR event and `platform.operation.reported.v1` in one transaction. Relational rows
SHALL store blob references, not raw bodies. A terminal completion transaction SHALL accept any
non-empty candidate set produced by the extraction path, so single-strategy extractions commit the
same facts as multi-candidate HTML runs. A quality rejection SHALL record the extraction path's
explicit failure class, and command intake SHALL record the parser version implied by the source
classification.

#### Scenario: a document succeeds

- **WHEN** fetched bytes are stored, parsed, and converted to shared Document IR
- **THEN** the database contains the completed owned facts and two due outbox rows, or contains none
  of that completion transaction

#### Scenario: a single-candidate extraction succeeds

- **WHEN** a direct PDF run commits its one selected `direct_pdf` candidate
- **THEN** the candidate, artifact, and outbox facts commit atomically exactly as a three-candidate
  HTML run commits its own

#### Scenario: a PDF-classified source is queued

- **WHEN** a capture command for a `.pdf` path is ingested
- **THEN** the queued run records the PDF parser version while every other source records the HTML
  parser version

### Requirement: Publication is bounded and acknowledged

The outbox publisher SHALL claim rows with an expiring lease and SHALL mark a row published only
after JetStream acknowledges it. Failures SHALL use bounded backoff and a finite dead-letter limit.
The consumer, worker, and publisher SHALL stop through the process cancellation tree and SHALL be
joined before shutdown completes.

#### Scenario: the broker does not acknowledge

- **WHEN** publication fails or receives no JetStream acknowledgement
- **THEN** the outbox row remains unpublished and becomes due again after bounded backoff
