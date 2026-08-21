## Purpose

Defines how one parsed HTML document produces several bounded candidates and one explainable,
deterministic quality decision before shared Document IR is published.

## ADDED Requirements

### Requirement: One DOM produces three bounded candidate strategies

Extractor SHALL run semantic, readability-compatible, and text-density strategies against the same
bounded DOM. Each strategy SHALL return ordered supported blocks plus the evidence needed for quality
evaluation, and SHALL NOT fetch, parse, or render the source again.

#### Scenario: a noisy article produces several candidates

- **WHEN** one HTML document contains navigation, an article body, related links, and a footer
- **THEN** the extraction attempt records semantic, readability-compatible, and text-density
  candidates derived from one DOM and no additional request

### Requirement: Quality decisions are deterministic and explainable

Each candidate SHALL receive bounded quality components for text volume, paragraph distribution,
link density, boilerplate density, and title agreement. The same candidate evidence and evaluator
version SHALL always produce the same total score, reasons, acceptance result, and winner. Equal
scores SHALL use a documented stable strategy order.

#### Scenario: evaluation is repeated

- **WHEN** the same candidate set is evaluated several times with the same evaluator version
- **THEN** every component, total score, reason, acceptance result, and selected strategy is equal

### Requirement: Low-quality HTML is refused

A candidate SHALL meet both the minimum content volume and total-score thresholds before it can
become Document IR. If no candidate qualifies, Extractor SHALL fail the run with a bounded quality
classification and SHALL NOT publish a successful document event.

#### Scenario: a login shell has no acceptable article

- **WHEN** every candidate contains only a short login or consent shell
- **THEN** the extraction fails as low quality and no successful Document IR is published

### Requirement: Candidate decisions are durable

Extractor SHALL persist each candidate strategy, evaluator version, bounded metrics, score, reasons,
and selected marker under the owning extraction run before completion is reported.

#### Scenario: a completed extraction is inspected

- **WHEN** an extraction selects one of several acceptable candidates
- **THEN** its owned records identify every evaluated candidate and exactly one selected candidate

### Requirement: Threshold changes are checked against a calibration corpus

The initial evaluator and every later threshold or weight change SHALL pass a committed synthetic
corpus that covers a semantic article, boilerplate-heavy HTML, malformed HTML, and a low-quality
shell. Corpus expectations SHALL name the accepted strategy or rejection and a bounded score range.

#### Scenario: an evaluator weight changes

- **WHEN** a score weight or acceptance threshold changes
- **THEN** the corpus test reports every changed winner, rejection, or expected score range
