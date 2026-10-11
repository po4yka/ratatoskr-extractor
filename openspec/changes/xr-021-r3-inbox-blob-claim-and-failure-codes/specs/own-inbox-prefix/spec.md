# own-inbox-prefix Delta

## ADDED Requirements

### Requirement: Every extractor connection replies on its own inbox prefix

The extractor's outbox publisher and capture consumer SHALL connect with the inbox prefix `_INBOX_extractor`, and the browser worker SHALL connect with `_INBOX_extractor_browser_worker`. The shipped stanza of each identity SHALL subscribe to exactly `<prefix>.>` and to nothing else.

#### Scenario: a JetStream request under the narrow subscribe grant

- **WHEN** the publisher fetches from the capture durable on a broker that lets the extractor identity subscribe only to `_INBOX_extractor.>`
- **THEN** the fetch completes

#### Scenario: a key-value read under the narrow subscribe grant

- **WHEN** the worker reads its completions bucket on a broker that lets the worker identity subscribe only to `_INBOX_extractor_browser_worker.>`
- **THEN** the read completes
