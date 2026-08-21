# HTML parse-once Document IR

## Purpose

Defines bounded parse-once conversion from verified HTML into deterministic shared Document IR with
ordered content and source-blob provenance.

## ADDED Requirements

### Requirement: One bounded DOM produces deterministic shared Document IR

Extractor SHALL parse verified HTML bytes once with a standards-compatible HTML5 parser and SHALL
construct `ratatoskr-contracts` Document IR from that one DOM. It SHALL preserve heading and
paragraph reading order, attach source-blob provenance to every block, and calculate the content
digest over the contracts repository's canonical JSON rendering of the ordered blocks.

#### Scenario: malformed HTML has one stable representation

- **WHEN** the same malformed HTML bytes are processed more than once
- **THEN** every result has the same ordered blocks, provenance, and content digest

#### Scenario: parser resources exceed policy

- **WHEN** input size or parsed node count exceeds its configured finite limit
- **THEN** parsing fails before any Document IR or derived artifact is published

### Requirement: Item 4 does not implement candidate scoring

The primitive HTML conversion SHALL use one fixed named strategy and SHALL NOT create competing
candidates, scores, quality thresholds, or browser escalation decisions.

#### Scenario: one DOM is not raced through speculative extractors

- **WHEN** HTML is converted during plan item 4
- **THEN** one parser invocation produces the primitive blocks without another network request or DOM
