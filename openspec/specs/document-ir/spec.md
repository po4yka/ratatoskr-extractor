# HTML parse-once Document IR

## Purpose

Defines bounded parse-once conversion from verified HTML into deterministic shared Document IR with
ordered content and source-blob provenance.

## Requirements

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

### Requirement: One parsed DOM feeds every HTML candidate

Extractor SHALL parse verified HTML bytes once and SHALL build every extraction candidate from that
same bounded DOM. Only the selected candidate SHALL become shared Document IR, and its provenance
SHALL name the selected strategy.

#### Scenario: candidate selection preserves parse-once behavior

- **WHEN** semantic, readability-compatible, and text-density candidates are evaluated
- **THEN** one parser invocation produces the DOM and the selected Document IR names the winning
  strategy in every block provenance entry
