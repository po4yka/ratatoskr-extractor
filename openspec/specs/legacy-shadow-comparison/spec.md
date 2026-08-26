# legacy-shadow-comparison Specification

## Purpose

Provides reproducible, offline evidence for approving or withholding each source-class cutover independently of the retired extraction monolith.

## Requirements

### Requirement: Shared offline samples compare legacy and Extractor results

Extractor SHALL evaluate each committed shadow sample against a provenance-pinned legacy observation and the current deterministic extraction path without network access. Every sample SHALL identify one source class and retain the original source address, source bytes or structured source input, and the legacy archive revision that produced its observation. A missing or unsupported current extraction result SHALL remain an explicit comparison outcome rather than being converted to an empty successful document.

#### Scenario: A committed sample is evaluated read-only

- **WHEN** the shadow comparison runs over the committed sample set
- **THEN** it emits one result for every sample without changing source, legacy-observation, or expected-result files

#### Scenario: A current source class is not implemented

- **WHEN** a sample belongs to a source class with no current Extractor path
- **THEN** the result records the current outcome as unsupported and the class cannot receive an approve verdict

### Requirement: Per-class cutover verdicts are explainable and independent

The comparison SHALL aggregate results separately for web articles, YouTube transcripts, and X posts. For each class it SHALL report legacy and current extraction success rates, a normalized content-overlap metric for jointly successful samples, and Document IR block counts by block kind. It SHALL apply committed, reviewable criteria independently: current success rate MUST be at least the legacy rate, every jointly successful sample MUST meet the class content-coverage threshold, and every legacy-success/current-failure sample MUST prevent approval. The report SHALL label each class `approve`, `hold`, or `insufficient-evidence` and state the failed criterion or sample names.

#### Scenario: One class passes without approving another

- **WHEN** web samples meet all criteria while X samples include an unsupported current result
- **THEN** the report recommends `approve` only for web articles and `hold` for X posts

#### Scenario: Content regression is visible

- **WHEN** a jointly successful current result has overlap below its class threshold
- **THEN** the report names the sample, records the metric, and withholds approval for that class

### Requirement: The report is a measurement, not a traffic switch

Running comparison or generating its report SHALL NOT alter production routing, enable a source class, invoke the legacy archive, or write to a database, event bus, or external service. The report SHALL state that owner approval and a separate cutover change are required before any traffic change.

#### Scenario: Report generation has no cutover side effect

- **WHEN** the offline comparison command completes
- **THEN** its only output is the requested report file or standard output and no source-class routing configuration changes
