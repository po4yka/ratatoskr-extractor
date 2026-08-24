# Design: Provider post-resolution

## Context

The conversion half of the adapters exists: `services/extractor/src/pipeline.rs` routes Hacker News and Reddit runs into `complete_provider`, which performs one native-JSON fetch through safe-fetch, stores the raw blob, and converts it in a blocking task via `crates/providers` (`from_algolia`, Reddit conversion) into `PostingData` blocks. Three constraints in the current checkout shape this design:

- `schema.sql` gives `extractor.fetches.run_id` a `UNIQUE` constraint, so one run can hold only one fetch row, and `extractor.artifacts` enforces `unique (run_id, kind)`, so one run can hold only one `raw_source` blob. Both collide with a run that performs two network operations (provider JSON plus resolved article).
- The provider endpoints are built inline (`https://hn.algolia.com/api/v1/items/{id}` in `crates/providers/src/lib.rs`) with no origin override, so hermetic end-to-end tests cannot reach a scripted server without a seam.
- `crates/safe-fetch` admission controls concurrency only (global and per-host permits); there is no time-based rate limit, and `FetchConfig` has no setting for one.

See proposal.md - Why for motivation and the specs in this change for the binding requirements.

## Goals / Non-Goals

**Goals:**

- Link posts resolve to their canonical external article URL inside the same run and continue through exactly one ordinary retrieval/extraction pass on that target.
- Self-text conversion keeps its current single-request shape and output.
- Provider response-content failures degrade gracefully: one typed failure class, then exactly one generic HTML attempt on the original normalized URL.
- Every resolution step persists as an extractor-owned fact committed atomically with the run's terminal state.
- Safe-fetch gains per-host minimum request spacing bounded by the caller's existing deadline.
- Hermetic end-to-end tests cover classification, adapter, resolution, fall-through, and pacing without live network access.

**Non-Goals:**

- No YouTube, X, authentication, or additional hosts (see proposal scope).
- No change to browser escalation triggers, quality scoring thresholds, cache keys, or conditional-request policy; the resolved target flows through the existing ordinary path untouched.
- No cross-repository contract changes; Document IR and bus subjects are unchanged.
- The resolution hop bound is a structural constant, deliberately not configuration.
- No LLM involvement anywhere in this path.

## Decisions

### D1: Re-entry happens in-process inside `process_run`, not via a second run

When the converted post carries an external link, `complete_provider` resolves it, validates the target under the full URL policy (same checks as any classified URL: scheme allowlist, host/port policy, DNS and prohibited-range screening) **before** sending any request, swaps the fetch target, and continues the normal sequence - safe-fetch, blob store, parse-once, candidates, deterministic scoring - within the same run and transaction boundary as before.

Alternatives considered:

- Publishing an intermediate event and re-enqueueing a fresh capture command for the target: rejected. It would create two runs, split provenance across records, add a bus round-trip, and contradict the fetch-once intent for what is logically one user-visible capture.
- Handing the resolved URL back to the classifier for full re-classification: rejected. The target must be treated as an ordinary URL (HTML or PDF), which the existing post-provider branch already does; routing it back through provider classification could loop and would spend the hop budget on classification rather than content.

Identity and provenance stay stable: the run keeps its `document_id`; the original capture URL remains on `extraction_runs`; the second completed fetch supplies the resolved final URL recorded with the document. The completion keeps the provider strategy name (`hacker_news_item` / `reddit_post`) so quality telemetry still shows where the content came from, while the fetch rows show the two-step chain.

Link detection: a non-empty external `url` field on the converted post means link post; otherwise the body/self-text converts directly as today. A URL pointing back at the source host's own item page is treated as self-contained (self-loop guard), preventing resolve-to-self cycles.

The hop bound ("one provider operation, then at most one more network operation") lives as a structural constant in the pipeline. It is the spec's behavioural guarantee; making it tunable would let a deployment silently reintroduce unbounded chasing chains.

### D2: Fall-through triggers are exactly the two response-content failure classes

Per the modified `provider-adapters` requirement "Non-JSON and invalid schemas fail explicitly", the generic HTML fallback fires only when the provider fetch returns a non-JSON media type or the schema conversion fails. Transport errors, timeouts, DNS failures, and policy blocks during the provider fetch terminate the run unchanged (existing `fail_run` behaviour), because they say nothing about whether the original URL itself is retrievable. Quality rejection after a successful fallback passes through the ordinary evaluator and stays terminal. An unmappable URL never reaches the provider at all and keeps today's straight-to-HTML behaviour.

The fallback performs exactly one ordinary fetch of the original normalized URL through the standard candidates and evaluator, so an anti-bot shell cannot masquerade as extracted content - it simply fails quality like any other low-value page. If that one attempt also fails, the run terminates with a diagnostic recording both outcomes (provider failure class plus fallback outcome).

### D3: Resolution steps persist in a dedicated table, committed with terminal facts

New extractor-owned table:

```sql
create table extractor.provider_resolutions (
    step_id uuid primary key,
    run_id uuid not null references extractor.extraction_runs (run_id),
    ordinal integer not null,
    kind text not null check (kind in ('provider_attempt', 'resolved_target', 'html_fallback')),
    outcome text,
    failure_class text,
    resolved_url text,
    created_at timestamptz not null default now()
);
```

Each row records one decision point: the provider attempt (outcome or failure class), the resolved target (URL), and the HTML fallback (outcome). `fetches` stays what it is - one row per network operation - so the two tables answer different questions (what bytes arrived vs. why the pipeline chose its path).

Consequently `fetches.run_id` loses its `UNIQUE` constraint: a resolved link post legitimately performs two fetches. The application already writes one fetch per operation; the constraint removal makes multiplicity representable, and tests assert the expected fetch counts.

Terminal writes stay atomic by extending the existing eventing terminal functions (`complete_document`, `fail_run`, `reject_quality`) to accept a slice of resolution steps inserted in the same transaction as the run-state transition. A parallel "commit steps separately" API was considered and rejected: the spec requires steps and terminal facts to commit together, and a two-call API invites partial states. Non-provider runs pass an empty slice, so call sites outside the provider path are mechanically unchanged.

Related schema relaxation: `artifacts` drops `unique (run_id, kind)` because a resolved link run stores two `raw_source` blobs (provider JSON and article bytes). `insert_artifact` stops upserting and inserts plainly; single-artifact kinds (`document_ir`, `diagnostics`) remain effectively unique through the linear run flow, asserted by tests.

### D4: Per-host pacing is hand-rolled minimum spacing, not a token bucket

Safe-fetch admission gains a per-host entry holding `next_allowed_start`. Claiming a request slot computes `start_at = max(now, *next_allowed)` and advances the entry to `start_at + interval` before acquiring the concurrency permit. The waiter sleeps until `start_at`, capped by the caller's remaining absolute deadline - pacing can delay a request but never extends the operation budget; exceeding the deadline yields the existing deadline error class.

The reservation precedes permit acquisition on purpose: spacing guarantees a lower bound between request starts, while permit contention may delay the actual start further. That matches the spec wording (requests spaced at least the configured interval apart, within the deadline) without coupling the two mechanisms.

Alternative considered: the `governor` crate's token bucket. Rejected because its steady-state semantics (refill rate, burst capacity) differ from the spec's exact inter-request-start spacing, and mapping one onto the other adds a dependency for roughly thirty lines of stateless-per-host arithmetic we already isolate behind the admission gate. This deviates from the general prefer-established-libraries rule deliberately; the rationale lives here and in the code comment.

Interval zero (default) disables pacing entirely, keeping the existing suite fast.

### D5: Configuration carries one integer, wired once

`FetchConfig` gains `per_host_min_interval_ms: u64` (default 0 = disabled). `compose.yaml` sets a production value. Tests that exercise pacing set a small interval (tens of milliseconds) on their own client instances; nothing else reads the field.

### D6: Test seams make the hardcoded endpoints injectable

Provider endpoints stay hardcoded for production but move behind a function taking a base origin, defaulted to the current constants (`https://hn.algolia.com/api/v1`). The `doc(hidden)` test-only entry point (`_complete_provider_for_test` extended, or a sibling) accepts explicit addresses, letting the HN link-post end-to-end test point the provider operation at one scripted server (recorded Algolia JSON fixture) and the resolved-target operation at a second scripted server (recorded article HTML), both covered by the resolver allowlist. Existing conversion tests continue exercising pure functions; the new tests follow the `pdf_pipeline.rs` harness pattern (`ScriptedServer`, `ScriptedResolver`, `TemporaryBlobRoot`, `TestDatabase`).

## Risks / Trade-offs

- [Fall-through can double requests to hosts whose provider APIs fail] -> Exactly one fallback attempt is a structural bound, the provider failure class and fallback outcome are both recorded, and pacing caps the request rate per host.
- [Dropping `fetches.run_id` UNIQUE weakens a database-level invariant] -> Application logic still writes one fetch row per network operation; resolution rows make the multi-hop case auditable; end-to-end tests assert exact fetch counts per scenario.
- [Two `raw_source` artifacts per run may surprise consumers reading artifacts naively] -> Artifact consumers key on `(run_id, kind)` pairs today; the change is extractor-local, and the resolution table documents which blob came from which step. Blob references published on the bus are unchanged.
- [A policy-blocked resolved target terminates the run instead of degrading to the discussion page] -> Accepted: policy violations must be loud and typed, and adding an extra fallback hop beyond the spec's single-fallback bound would blur the hop guarantee. Rare in practice; visible in diagnostics.
- [Pacing sleeps interact badly with shutdown or tight deadlines] -> Sleeps are bounded by the caller's remaining deadline and cancelled with the operation; spacing zero disables the feature; the dedicated pacing test drives both the spacing and deadline-exceeded paths.
- [Injectable endpoint origins widen the test surface into production code] -> The override lives behind `doc(hidden)` test-only constructors with production defaults; regular builds never parametrize it.

## Migration Plan

Development status applies: edit `schema.sql` in place (drop the two UNIQUE constraints, add `extractor.provider_resolutions`), recreate the development database from the definition, and restart the compose stack. No migration ledger, no backfill - no database holds data that must survive the change. Rollback is reverting the schema edit and recreating the database likewise. Documentation updates (README status wording, `docs/IMPLEMENTATION_PLAN.md` item annotation, DEVELOPMENT notes if commands changed) land in the same change.

## Open Questions

None blocking. Whether resolved-target fetches need distinct cache-key treatment when caching lands later is deferred to the caching capability; it does not affect these specs, this approach, or the task breakdown.
