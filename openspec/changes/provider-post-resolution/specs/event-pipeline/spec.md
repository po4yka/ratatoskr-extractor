# event-pipeline Delta

## MODIFIED Requirements

### Requirement: Owned facts and outgoing messages commit together

Extractor SHALL persist run, fetch, artifact, candidate, and resolution-step facts only under
`extractor.*`. Each network operation performed for a run SHALL record its own fetch row. A
successful primitive extraction SHALL commit its run/result records and durable outbox rows for the
shared Document IR event and `platform.operation.reported.v1` in one transaction. Relational rows
SHALL store blob references, not raw bodies. A provider resolution SHALL commit its ordered
resolution steps atomically with the run's terminal facts. A terminal completion transaction SHALL
accept any non-empty candidate set produced by the extraction path, so single-strategy extractions
commit the same facts as multi-candidate HTML runs. A quality rejection SHALL record the extraction
path's explicit failure class, and command intake SHALL record the parser version implied by the
source classification. Claimed runs SHALL carry their source classification so the pipeline can
route them without a second database read. An escalated run SHALL renew its lease while awaiting a
render result and SHALL terminate with the carried worker failure class when rendering fails.

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

#### Scenario: an escalated run survives its lease window

- **WHEN** a render result arrives after the original lease would have expired
- **THEN** the run remains claimable by its owner until the render budget elapses, and its terminal
  facts commit exactly once

#### Scenario: a resolved link post commits its resolution trail

- **WHEN** a provider-classified link run resolves an external article and completes
- **THEN** the terminal transaction contains the provider fetch, the resolved-target fetch, the
  ordered resolution steps, and the completion facts, or none of them

#### Scenario: a fallen-back run records both attempts

- **WHEN** a provider fetch fails its schema and the generic HTML attempt is rejected on quality
- **THEN** the terminal facts include the provider failure class, both fetch rows, and the
  rejection reason in one transaction
