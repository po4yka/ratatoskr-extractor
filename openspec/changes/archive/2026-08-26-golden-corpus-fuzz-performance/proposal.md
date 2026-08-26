## Why

The extractor's deterministic pipeline is currently protected only by small, crate-local fixtures
and ad-hoc live smoke scripts. It needs committed, offline evidence that pins end-to-end Document
IR outputs, exercises hostile parser input continuously, and makes resource regressions visible
before deployment to the single constrained host.

## What Changes

- Add an offline, licensed/synthetic golden corpus for HTML, PDF, provider JSON, and transcript
  inputs, with canonical expected Document IR JSON and a read-only verification mode.
- Add an explicit `--bless` corpus command that is the only way to rewrite expected outputs and
  requires the resulting fixture diff to be reviewed.
- Add bounded cargo-fuzz targets and seed corpora for HTML parsing, PDF extraction, and URL
  normalization/classification; run each target for a finite smoke budget in CI.
- Add a reproducible performance-report command that measures corpus throughput, latency, and
  peak resident memory; commit its baseline and reject values beyond its documented thresholds.
- Document the new local/CI commands and mark implementation-plan item 9 as implemented.

## Capabilities

### New Capabilities

- `extraction-quality-assurance`: Offline corpus, fuzz, and performance evidence required for
  deterministic extraction paths.

### Modified Capabilities

- None. Existing extraction contracts remain unchanged; this change adds their executable quality
  evidence without changing their externally observable semantics.

## Impact

Adds a small test-only workspace crate, synthetic fixtures and expected outputs, a standalone
`fuzz/` cargo-fuzz package, CI smoke steps, a committed performance baseline/report, and developer
documentation. It does not change shared contracts, database schema, network behavior, or the
ordinary fetch-once/parse-once pipeline.
