## Context

See `proposal.md` and the `legacy-shadow-comparison` delta. The retired monolith is a read-only local archive, while CI must run without it or live network access. Existing corpus inputs already exercise deterministic HTML and YouTube conversion but have no legacy baseline or source-class decision artifact.

## Goals / Non-Goals

**Goals:**

- Compare current extraction with a reproducible legacy observation for the same committed sample.
- Make content coverage, outcome differences, block-shape differences, criteria, and verdicts auditable in one deterministic report.
- Keep the X class in the report even though its current route is intentionally absent.

**Non-Goals:**

- Executing the legacy repository in CI, changing it, or reproducing its provider chain.
- Implementing X extraction, altering fetch/routing, or turning a favorable verdict into a traffic switch.
- Defining a universal semantic-equivalence score; the report measures normalized word-token coverage.

## Decisions

### Store captured legacy observations, not a legacy runtime dependency

Each fixture records the legacy archive revision, capture provenance, success state, and normalized plain-text/block representation. The offline command runs current Extractor conversion from the same source input and compares it with that immutable observation. This is the only way to keep CI hermetic while retaining a reviewable baseline from the archive.

An alternative was invoking the archive from the harness. It is rejected because its Python dependency/service provider graph and live providers make the result non-hermetic, and the archive is explicitly read-only.

### Keep comparison source-specific behind a small corpus fixture interface

The corpus crate owns a typed source enum that dispatches existing deterministic HTML and YouTube converters. An explicit unsupported source result represents a delegated source class such as X; it is evidence, not an adapter or fallback. The harness never fetches, parses a second DOM, or changes production code paths.

### Use deterministic, transparent metrics and conservative criteria

Success is a typed successful Document IR outcome. Content overlap is the fraction of unique normalized legacy word tokens retained by the current result; it is directional so that missing baseline content is visible. Block statistics count Document IR block kinds. A class approves only with a non-inferior success rate, no legacy-success/current-failure cases, and every jointly successful case at/above the class threshold. Classes without at least one legacy-success sample are `insufficient-evidence`; every other failed condition is `hold`.

The alternative of averaging coverage is rejected: a single severe regression could be hidden by high-overlap samples.

### Commit a generated expected report and verify it read-only

The report command can write a requested file; the verification API renders bytes in memory and compares them with a committed report. This follows the existing corpus bless/verify separation and makes metric or criterion changes reviewable.

## Risks / Trade-offs

- [Captured observations can become stale as legacy provenance changes] → fixtures pin the archive revision and capture path; a refresh is an explicit reviewed update.
- [Small synthetic sample set cannot prove fleet-wide parity] → verdicts state sample counts and use `insufficient-evidence` until a class has the committed evidence required by its criteria.
- [Plain-text overlap cannot assess ordering or semantics] → report retains IR block statistics and names case-level results; future corpus expansion can add metrics without changing routing.

## Migration Plan

1. Add the offline harness, source fixtures, criteria, expected report, and verification gate.
2. Review a favorable class report separately from implementation and request owner approval.
3. If approved, create a separate routing/cutover change; reverting this change only removes measurement evidence and cannot affect traffic.
