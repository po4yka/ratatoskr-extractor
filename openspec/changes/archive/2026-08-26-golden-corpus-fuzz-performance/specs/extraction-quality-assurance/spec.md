## Purpose

Defines offline, reproducible evidence that protects deterministic extraction quality and resource
budgets without a live-network dependency.

## ADDED Requirements

### Requirement: Committed corpus pins supported extraction outputs

Extractor SHALL verify a committed, offline corpus of licensed or synthetic source inputs against
canonical expected Document IR outputs. The corpus SHALL cover every currently supported Document
IR block kind and the HTML, direct-PDF, Hacker News, Reddit, and YouTube transcript conversion
paths. Verification SHALL be read-only by default and SHALL report the case name and a structural
diff when an expected result differs.

#### Scenario: An unchanged corpus verifies deterministically

- **WHEN** corpus verification runs twice against the same committed inputs
- **THEN** both runs accept every case and produce the same canonical Document IR bytes

#### Scenario: A changed extraction result is rejected

- **WHEN** a supported extraction path produces an output different from its committed expectation
- **THEN** corpus verification fails and identifies the mismatched case without rewriting any
  expected file

### Requirement: Golden updates require an explicit reviewable action

Extractor SHALL provide a separately invoked bless action to regenerate expected outputs from
committed corpus inputs. The ordinary verification command SHALL NOT bless implicitly, and the
bless action SHALL leave regenerated expected files as an ordinary reviewable working-tree diff.

#### Scenario: An intentional output change is blessed

- **WHEN** a maintainer invokes the documented bless action for a corpus case
- **THEN** only that case's committed expectation is rewritten and a subsequent read-only
  verification accepts it

### Requirement: Hostile parser and URL inputs are fuzzed with bounded time

Extractor SHALL maintain seeded, structure-aware fuzz targets for HTML parsing, direct PDF
extraction, and URL normalization/classification. Continuous integration SHALL run each target for
a finite configured duration and SHALL fail on a sanitizer finding, crash, timeout, or preserved
crash artifact.

#### Scenario: Seeded fuzz smoke completes

- **WHEN** the CI fuzz job runs each target with its committed seed corpus
- **THEN** every target completes within its configured duration without a finding

### Requirement: Corpus performance remains within recorded budgets

Extractor SHALL provide a reproducible report over the committed corpus that records throughput,
latency, and peak resident memory. The repository SHALL commit a baseline with explicit thresholds;
verification SHALL fail when the measured report exceeds a threshold, including the extractor's
768 MiB `MemoryHigh` budget from the deployment target.

#### Scenario: A report remains inside the baseline

- **WHEN** the corpus report runs under its verification mode
- **THEN** it writes a deterministic report and accepts measurements within the committed latency,
  throughput, and memory thresholds

#### Scenario: A resource regression exceeds its threshold

- **WHEN** a corpus report measurement exceeds an explicit baseline threshold
- **THEN** verification fails with the metric, observed value, and allowed value
