## Why

`SourceRoute::HackerNews` and `SourceRoute::Reddit` are classified at intake but still fall through
to generic HTML scraping, exactly the path repository rules say must not be used when an
authoritative structured representation exists. Both sources publish stable public JSON that
converts deterministically into Document IR, which makes them the first two provider adapters of
plan item 7's second half.

## What Changes

- Add a pure parsing crate `crates/providers` that converts Hacker News Algolia item documents and
  Reddit link-plus-comments listings into shared Document IR through one typed entry point, with
  synthetic fixtures modeled on the real API shapes.
- Map classified URLs to their provider-native representations before fetching: Hacker News item
  URLs resolve to the Algolia item endpoint and Reddit comment permalinks gain a `.json` suffix, so
  a provider run performs exactly one network operation.
- Route claimed runs by classification: provider-classified runs whose URL maps to a provider
  representation fetch that representation and parse it with the matching adapter; unmappable URLs
  fall back to the ordinary HTML path unchanged.
- Accept `application/json` responses on the provider branch, treat non-JSON or schema-invalid
  bodies as typed failures distinct from parse failures, and record `providers-v1` as the parser
  version for both routes at intake.
- Produce one selected candidate per provider run using the shared plain-text evaluator, publish
  the standard completion events, and keep the raw JSON artifact as `raw_source`.
- Keep YouTube transcripts, GitHub, X, Instagram, Threads, Telegram, and authenticated provider
  surfaces outside this change; delegated platforms stay with their owning services.

## Capabilities

### New Capabilities

- `provider-adapters`: Deterministic conversion of provider-native JSON into Document IR for
  classified Hacker News and Reddit URLs, including URL-to-representation mapping, bounded
  schemas, explicit non-JSON failure modes, and single-fetch semantics.

### Modified Capabilities

- `event-pipeline`: Claimed runs carry their source classification, and provider classifications
  record the `providers-v1` parser version at intake.

## Impact

New crate `crates/providers` (serde_json-tolerant, thiserror-typed) consumed by
`services/extractor`; `crates/eventing` extends `QueuedRun` and intake parser-version mapping;
`crates/core` gains provider budgets. No shared contract shape, database column, or event subject
changes; HTML, PDF, and unsupported-media behavior stay byte-for-byte identical.
