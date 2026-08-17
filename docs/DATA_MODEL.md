# Extractor data model

## Owned schema: `extractor.*`

Planned records:

- `sources`: owner, original/normalized/canonical URL, host, classification.
- `fetches`: status, headers subset, redirects, hashes, sizes, cache validators, timings.
- `artifacts`: raw body, rendered DOM, PDF, IR, diagnostics blob references.
- `extraction_runs`: versions, policy, lifecycle, operation/correlation, accepted candidate.
- `candidates`: extractor version, metrics, score, reasons, artifact relation.
- `host_strategies`: expiring measured strategy hints, never permanent bypasses.
- `outbox_events`, `inbox_events`.

## Constraints

Owner scope is mandatory. Content-addressed blobs are immutable. Run identity includes source/content/policy/algorithm versions. No raw body is stored directly in relational rows. Cross-schema writes/foreign keys are forbidden.

Retention separates raw artifacts, derived IR, diagnostics, failed staging, and privacy deletion. Migrations preserve replayability and document backfill/cutover.
