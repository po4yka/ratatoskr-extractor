## MODIFIED Requirements

### Requirement: Owned facts and outgoing messages commit together

Extractor SHALL persist run, fetch, artifact, and candidate facts only under `extractor.*`. A
successful primitive extraction SHALL commit its run/result records and durable outbox rows for the
shared Document IR event and `platform.operation.reported.v1` in one transaction. Relational rows
SHALL store blob references, not raw bodies. A terminal completion transaction SHALL accept any
non-empty candidate set produced by the extraction path, so single-strategy extractions commit the
same facts as multi-candidate HTML runs. A quality rejection SHALL record the extraction path's
explicit failure class, and command intake SHALL record the parser version implied by the source
classification. Claimed runs SHALL carry their source classification so the pipeline can route them
without a second database read.

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

#### Scenario: a provider-classified run is claimed

- **WHEN** a worker leases a run whose source was classified as a provider route
- **THEN** the claim carries that classification and the queued run recorded the providers parser
  version at intake
