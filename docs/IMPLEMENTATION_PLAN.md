# Extractor implementation plan

- [x] Scaffold Rust workspace, typed config, telemetry, errors, health, and test harness.
- [x] Implement URL normalization, classification, and SSRF policy.
- [x] Implement streaming safe fetch, redirects, cache metadata, and an extractor-owned raw artifact announced by `BlobRef`.
- [x] Implement HTML parse-once and Document IR primitives.
- [ ] Add semantic/readability/density candidates and deterministic quality evaluator.
- [x] Persist runs/candidates/artifacts and publish events with outbox/inbox.
- [ ] Add direct PDF and selected provider/source adapters.
- [ ] Add isolated browser worker and strict escalation policy.
- [ ] Build golden corpus, fuzzing, and performance reports.
- [ ] Run legacy shadow comparison and cut over source classes independently.

Items 1 through 3 deliberately contain no Document IR, database, migrations, command bus, inbox, or
outbox. Item 4 starts Document IR; item 6 adds persistence and bus integration.

Definition of Done: security limits, determinism, provenance, corpus quality, performance budgets,
retries, and workspace integration pass. Deferred: LLM navigation, broad authenticated sessions,
and multiple racing browser engines.
