## ADDED Requirements

### Requirement: Offline corpus includes legacy shadow evidence

Extractor SHALL keep the legacy shadow sample inputs, recorded legacy observations, criteria, and generated review report under version control beside the offline corpus. A gate SHALL regenerate the report from those committed inputs and fail when the report's semantic content differs from its committed expected report.

#### Scenario: Shadow evidence stays reproducible

- **WHEN** the shadow-report verification command runs twice against an unchanged checkout
- **THEN** each run produces identical report bytes and accepts the committed expected report

#### Scenario: A changed observation is reviewable

- **WHEN** a legacy observation, current result, or criterion changes the report
- **THEN** verification fails with the generated report available as a reviewable diff and does not overwrite the committed report
