# run-level-blob-claim Delta

## ADDED Requirements

### Requirement: A blob run verifies against its own command's claim

The owner, media type and length of a blob capture SHALL be stored on the run that the command queues, and the run SHALL be verified against that claim and no other.

#### Scenario: a corrected re-capture after a wrong length

- **WHEN** a digest is captured with a wrong length and the run fails `blob_mismatch`, and the digest is captured again with the right length
- **THEN** the second run succeeds

#### Scenario: a wrong claim for a digest that was extracted before

- **WHEN** an extracted digest is captured again with media type `image/png`, or with a wrong length
- **THEN** the run fails with class `unsupported_media`, or `blob_mismatch`

#### Scenario: two queued claims for one digest

- **WHEN** two commands for one digest with different claims are queued before either run is claimed
- **THEN** each run is judged against its own claim
