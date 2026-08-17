# Extractor implementation plan

1. Scaffold Rust workspace, typed config, telemetry, errors, health, and test harness.
2. Implement URL normalization, classification, and SSRF policy.
3. Implement streaming safe fetch, redirects, caching metadata, and raw BlobStore artifact.
4. Implement HTML parse-once and Document IR primitives.
5. Add semantic/readability/density candidates and deterministic quality evaluator.
6. Persist runs/candidates/artifacts and publish events with outbox/inbox.
7. Add direct PDF and selected provider/source adapters.
8. Add isolated browser worker and strict escalation policy.
9. Build golden corpus, fuzzing, and performance reports.
10. Run legacy shadow comparison and cut over source classes independently.

Definition of Done: security limits, determinism, provenance, corpus quality, performance budgets, retries, migrations, and workspace integration pass. Deferred: LLM navigation, broad authenticated sessions, and multiple racing browser engines.
