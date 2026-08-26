# Tasks: harden-browser-escalation-policy

## 1. Policy inputs in configuration

- [x] 1.1 Add failing tests `crates/core/tests/config.rs::render_policy_defaults_are_safe_and_fields_validate`: built-in defaults carry `enabled = false`, an empty `allowed_hosts`, and a finite nonzero `max_escalations_per_day`; validation rejects `max_escalations_per_day == 0` when enabled and rejects non-finite (zero) navigation/total budgets. Run it and confirm the new fields are absent today.
- [x] 1.2 Add `allowed_hosts` and `max_escalations_per_day` to `RenderConfig` with defaults, validation rules, and doc comments; make 1.1 pass alongside every existing config suite, run format/Clippy gates, and commit this pair.

## 2. The escalation decision function

- [x] 2.1 Add failing unit tests `services/extractor/src/escalation.rs` covering every gate combination of the pure decision input (quality rejection present, empty-shell evidence true/false, master switch on/off, host allowed/unlisted-with-nonempty-allowlist/empty-allowlist, day budget remaining/exhausted) asserting the exact decision outcome including named denial classes, plus a deny-by-default assertion when rendering is disabled. Run them and confirm the module does not exist today.
- [x] 2.2 Implement the deterministic decision type with documented inputs and outputs; replace the inline booleans in `services/extractor/src/pipeline.rs::complete_html` so every escalation passes through it, keep the existing positive path byte-identical in behaviour when all gates permit, make 2.1 pass, run service gates, and commit this pair.

## 3. Durable per-day render budget

- [ ] 3.1 Add failing database test (real PostgreSQL via the existing test harness): apply the current schema definition, call the new budget-slot consumer with a cap of one twice, and assert the first call reports consumed with count one while the second reports denied without advancing the counter; also assert the row appears keyed by the UTC day. Run it and confirm the table and function are absent today.
- [ ] 3.2 Edit `schema.sql` in place adding the `render_budgets(utc_day, escalated)` accounting table and implement one atomic SQL statement (conditional upsert-and-increment returning the new count) behind a typed eventing API used before publishing any render command; make 3.1 pass, run workspace gates, and commit this pair.

## 4. Extractor honours the gates end to end

- [ ] 4.1 Add failing integration tests beside the existing escalation scenario (same real PostgreSQL/JetStream/Chrome harness): (a) a non-empty `allowed_hosts` excluding the fixture host denies with the recorded denial reason and publishes zero render commands while leaving the day counter at zero; (b) seeding the counter at its cap denies identically without advancing it; (c) the shipped hydration-shell scenario still completes when the policy permits. Run them and confirm (a)/(b) currently publish render commands because the gates do not exist.
- [ ] 4.2 Wire the decision function and budget-slot consumption into the escalation path exactly as specified (deny records quality rejection plus the denial-class reason and publishes nothing; permit consumes the slot in the same step that precedes the command publication), make 4.1 pass alongside every existing suite, run service gates, and commit this pair.

## 5. Worker process recycling

- [ ] 5.1 Add failing transport test `services/browser-worker/tests/transport.rs::consumer_exits_after_the_configured_job_count`: a stub executor records invocations, two commands are published, the consumer runs with a finite per-process job limit of two, and the assertion requires the consumer future to return successfully after both terminal outcomes while both events published and both deliveries acknowledged; a third redelivered duplicate before the limit consumes no capacity. Run it and confirm the setting and the counting loop are absent today.
- [ ] 5.2 Add `max_jobs_per_process` to `WorkerSettings` (flat `BROWSER_MAX_JOBS_PER_PROCESS` environment name, finite default) and stop the consumer cleanly once that many jobs reach a terminal outcome, deduplicated deliveries excluded; make 5.1 pass, run worker gates, and commit this pair.

## 6. Documentation and delivery

- [ ] 6.1 Correct the stale current-phase text in `AGENTS.md` (provider adapters, PDF, and the isolated browser worker are implemented), document `BROWSER_MAX_JOBS_PER_PROCESS` and the new `RENDER__ALLOWED_HOSTS` / `RENDER__MAX_ESCALATIONS_PER_DAY` knobs beside their siblings in `DEVELOPMENT.md`, deployment environment examples, and align the README escalation paragraph. No test: documentation verified against the built binaries.
- [ ] 6.2 Run the exact DEVELOPMENT.md gate order including real PostgreSQL/JetStream/Chrome suites and the file-size ratchet, `openspec validate --archived --strict`, archive this change, integrate into `main`, push, and verify remote checks.
