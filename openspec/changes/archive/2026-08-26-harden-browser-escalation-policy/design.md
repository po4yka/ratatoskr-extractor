# Design: harden-browser-escalation-policy

## Context

The browser worker and the empty-shell escalation shipped in
`2026-08-23-add-browser-escalation-and-worker` with `render.enabled` as the only operator gate; its
design recorded host-strategy tables and escalation-storm caps as future work. This change is that
future work for cost control, kept inside one repository: no wire-format change, no new subjects,
no cross-repository contract touch.

## Goals / Non-Goals

Goals: one pure decision point for escalation with test-pinned gate combinations; host allowlist;
durable per-day spend ceiling; bounded worker process lifetime. Non-goals: per-host quality
strategy tables, cache-key changes, scoring-threshold movement, any browser-worker decision-making.

## Decisions

### One pure policy function in the extractor service

`services/extractor/src/escalation.rs` exposes a decision type evaluated from already-fetched
facts (quality rejection present, shell evidence, config gates). It returns permit or a named
denial class. Rationale: the pipeline currently folds four booleans inline, which is exactly what
the acceptance criteria call a state machine to test; a pure function gives an exhaustive unit
matrix without PostgreSQL/NATS/Chrome. Alternative considered - keeping inline booleans and adding
integration cases only - rejected because combinations grow multiplicatively and integration tests
cannot enumerate them cheaply.

### Allowlist semantics: empty means unrestricted

`allowed_hosts: Vec<String>` matches the final URL host exactly after ASCII lowercasing; an empty
list imposes no host restriction. Rationale: the archived spec's hydration scenario runs with only
`enabled = true`, and flipping empty to deny-all would silently disable shipped behaviour on
upgrade; restriction stays opt-in while the master switch remains the primary denial.
Alternative considered - empty denies all - rejected as a silent breaking change to an archived
capability; operators who want strict posture list their hosts explicitly.

### Durable day counter in the extractor schema

One table `render_budgets(utc_day date PRIMARY KEY, escalated integer NOT NULL)` edited into
`schema.sql`; before publishing a command the pipeline opens one transaction that reads the row for
the current UTC day, denies when `escalated >= max_escalations_per_day`, otherwise upserts
`escalated + 1` and commits before `request_render`. Rationale: the budget must survive extractor
restarts to be a cost control at all, and the check-and-increment must be atomic against
concurrent runs; PostgreSQL gives both with one statement pattern. In-process atomics rejected -
a restart resets them, which defeats the bound this change exists to add.

### Worker recycling by handled-job count

`BROWSER_MAX_JOBS_PER_PROCESS` (default 500) counts terminal outcomes handled by the consumer
loop; at the limit the loop returns cleanly, `main` exits 0, and compose/systemd restarts the
service with fresh Chromium. Rationale: job count is deterministic and portable where RSS probing
is platform-specific noise; cgroup memory/PID limits in the compose profile remain the actual leak
backstop, and recycling bounds the slow leaks cgroups tolerate. Deduplicated redeliveries do not
count, so a busy-but-idempotent queue cannot spin the process down.

### Production executor and supervisor limits

The binary constructs `ChromiumExecutor::launch` before accepting commands, passing the optional
typed `BROWSER_CHROME_BIN` value and retaining the production `NavigationPolicy` (loopback remains
denied). This removes the placeholder executor that could only return `browser_unavailable` while
leaving the test-only loopback policy confined to direct executor tests. The compose browser profile
sets a finite CPU cap alongside its existing memory and PID caps, and uses `restart: always` so a
clean job-budget exit or cgroup-enforced termination receives a fresh worker. No extractor database
configuration enters this service.

### Denial recording rides existing columns

A denied escalation records the ordinary quality rejection plus the run failure/diagnostic reason
string (`render_policy_denied:<class>`), reusing the terminal-transition paths already present. No
new event subject, no schema addition beyond the counter table. Alternative considered - a new
diagnostics artifact - rejected as surplus surface for information the run record already carries.

## Risks / Trade-offs

[Allowlist exact-match misses subdomain families] → documented v1 semantics; widening to suffix
matching is a config-visible follow-up, not a format change.
[Day-boundary races] → the transaction keys on the UTC day of evaluation; a command published at
23:59:59.9 counts against that day even if the worker renders next day; acceptable for a cost
ceiling.
[Worker exits between deliveries] → supervisor restart cost lands once per N jobs; default 500
keeps amortized launch cost negligible against per-page render seconds.
[Chromium cannot launch] → startup exits nonzero before the consumer admits a command; the
supervisor retries under the same process limits instead of publishing a misleading worker failure.
[Counter table growth] → one row per UTC day; bounded by deployment lifetime and trivially
prunable alongside existing retention work later.

## Migration Plan

Deploy order does not matter: new fields carry safe defaults, the counter table appears with the
next database creation from `schema.sql`, and the worker knob defaults finite. Rollback is config
reversion (`enabled = false`); no persisted extraction state changes meaning.

## Open Questions

None blocking.

### Events-stream ownership (added during apply)

Applying this change exposed a latent ordering bug outside the original scope: the worker's setup
created the shared event stream with render-only subjects, so any fresh broker that saw a worker
before an extractor narrowed the stream for every later publisher. The fix keeps creation with the
extractor's publisher (`evt.>` bounded config) and turns the worker's setup into an existence check
with an explanatory error. Recorded here because it changes deployment expectations: a standalone
worker can no longer bootstrap an empty broker alone.
