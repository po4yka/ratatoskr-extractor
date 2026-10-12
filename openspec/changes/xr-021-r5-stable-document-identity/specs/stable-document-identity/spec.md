# stable-document-identity Delta

## ADDED Requirements

### Requirement: A document identity is derived from its owner and its content digest

The extractor SHALL assign every Document IR a `document_id` that is a pure function of the owner of the capture and the content digest of the document, and SHALL NOT mint an identity per run.

#### Scenario: the derivation is deterministic and separated

- **WHEN** `stable_document_id` is called twice with one owner and one digest
- **THEN** both calls return the same RFC 9562 version 8 identifier, and a call with another owner or another digest returns a different one

#### Scenario: assembling the same blocks twice

- **WHEN** `assemble_document` is called twice with the same blocks and the same owner
- **THEN** both documents carry the same `document_id`, and a call with another owner carries a different one

### Requirement: Two operations over the same bytes share one document

Two captures of the same text for the same owner SHALL each complete as their own operation and SHALL name one document.

#### Scenario: one blob captured twice

- **WHEN** one owner sends the same bytes in two capture commands
- **THEN** both runs succeed, both `evt.content.document.extracted.v1` events carry the same `document.document_id` and the same `content_digest`, the two operation reports name the same `document:<id>`, and there are two distinct event ids and two operations

#### Scenario: the same PDF arrives by two routes

- **WHEN** the same PDF bytes arrive once as a blob command and once as a URL fetched from a local server
- **THEN** both runs succeed and the two events carry one `document_id`

#### Scenario: different text or another owner

- **WHEN** a PDF with different text is captured by the same owner, or the same PDF is captured by another owner
- **THEN** the event carries another `document_id`

### Requirement: A run carries its owner and no identity

A claimed run SHALL expose the owner of the capture and SHALL NOT expose or store a document identity.

#### Scenario: a claimed run

- **WHEN** a queued run is claimed
- **THEN** `QueuedRun.owner` is the owner of the command, and the `extraction_runs` table has no `document_id` column
