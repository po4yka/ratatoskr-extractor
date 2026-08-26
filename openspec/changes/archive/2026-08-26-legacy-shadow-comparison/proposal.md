## Why

Ratatoskr Extractor needs reproducible evidence before any source class can replace the retired monolith. The monolith's multi-provider result is a quality baseline, but it cannot be part of the production path or a live-network gate.

## What Changes

- Add an offline shadow-comparison harness that evaluates committed, provenance-pinned legacy observations and current Extractor output for the same sample.
- Produce a deterministic per-source-class report with success rate, normalized content overlap, and Document IR block statistics.
- Define explicit independent cutover criteria and a recommendation verdict for web articles, YouTube transcripts, and X posts.
- Keep the comparison measurement-only: it changes neither routing nor production traffic.

## Capabilities

### New Capabilities

- `legacy-shadow-comparison`: Offline comparison evidence and independent source-class cutover recommendations.

### Modified Capabilities

- `extraction-quality-assurance`: The committed quality corpus gains reviewable legacy-baseline comparison evidence.

## Impact

- Affects `tools/corpus`, its committed synthetic inputs and expected observations, the offline CI gate, and Extractor documentation.
- The legacy archive at `/Users/po4yka/GitRep/ratatoskr-repositories/ratatoskr` is inspected only; it is not modified or executed in CI.
- No public API, database schema, dependency, production route, or cross-repository contract changes.
- Rollback is deletion/reversion of this measurement tooling; no traffic is switched or state migrated.
