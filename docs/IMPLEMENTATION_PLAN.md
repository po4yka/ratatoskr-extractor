# Extractor implementation plan

- [x] Scaffold Rust workspace, typed config, telemetry, errors, health, and test harness.
- [x] Implement URL normalization, classification, and SSRF policy.
- [x] Implement streaming safe fetch, redirects, cache metadata, and an extractor-owned raw artifact announced by `BlobRef`.
- [x] Implement HTML parse-once and Document IR primitives.
- [x] Add semantic/readability/density candidates and deterministic quality evaluator.
- [x] Persist runs/candidates/artifacts and publish events with outbox/inbox.
- [x] Add direct PDF extraction producing Document IR with typed encrypted/pathological failure modes.
- [x] Add selected provider/source adapters (Hacker News, Reddit).
- [x] Resolve provider link posts through one ordinary retrieval pass and fall back once on typed provider response-content failures.
- [x] Add isolated browser worker and strict escalation policy.
- [x] Build golden corpus, fuzzing, and performance reports.
- [x] Run legacy shadow comparison and cut over source classes independently.

Items 1 through 3 deliberately contain no Document IR, database, command bus, inbox, or outbox.
Item 4 starts Document IR, item 5 selects deterministic HTML candidates, and item 6 adds persistence
and bus integration.

Definition of Done: security limits, determinism, provenance, corpus quality, performance budgets,
retries, and workspace integration pass. Deferred: LLM navigation, broad authenticated sessions,
and multiple racing browser engines.
