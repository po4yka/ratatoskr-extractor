# render-pipeline Delta

## MODIFIED Requirements

### Requirement: Escalation is deterministic, single-shot, and reuses the HTML path

When a direct HTML extraction rejects low-quality content, Extractor SHALL decide escalation only
through one deterministic policy evaluation that requires every gate to permit: the raw bytes match
empty-shell evidence with near-zero extracted text, rendering is enabled, the final URL host is
allowed by the configured host allowlist (an empty allowlist imposes no host restriction beyond the
other gates; a non-empty allowlist matches hosts exactly, case-insensitively), the per-UTC-day
escalation budget has remaining capacity, and the render budgets are finite. The policy SHALL deny
by default when any gate fails, and a denial SHALL record why on the run while publishing zero
render commands. When the policy permits, Extractor SHALL publish exactly one render command
derived from the run inside the same transaction that consumes one unit of the day budget, renew
the run lease while awaiting the result within the render budget, then either re-parse the returned
DOM through the ordinary parser, candidates, evaluator, provenance rules naming the rendered
artifact, and standard events, or terminate the run with the worker's carried failure class. The
escalated parse SHALL have no escalation branch of its own.

#### Scenario: a hydration shell completes from rendered DOM

- **WHEN** the direct fetch returns an empty shell, the policy permits (rendering enabled, host allowed, budget remaining), and the escalated render returns hydrated HTML
- **THEN** the run succeeds with provenance naming the rendered artifact, the same evaluator gates as direct extraction, exactly one render command, the day counter advanced by one, and the expected total request count

#### Scenario: a worker timeout carries through

- **WHEN** the render job fails with `navigation_timeout`
- **THEN** the run terminates with that class and no document event exists

#### Scenario: a host outside the allowlist never reaches the worker

- **WHEN** the direct fetch returns an empty shell of low-quality content whose host is absent from a non-empty `allowed_hosts`, with rendering enabled and budget remaining
- **THEN** no render command is published, the run records the quality rejection with the denial reason, and the day counter does not advance

#### Scenario: an exhausted daily budget denies without spending the worker

- **WHEN** the day counter already holds the configured maximum and another empty shell of a permitted host is rejected by quality
- **THEN** no render command is published, the run records the quality rejection with the denial reason, and the counter value stays at the maximum

#### Scenario: concurrent runs cannot exceed the daily budget

- **WHEN** exactly one unit of budget remains and two permitted escalations decide concurrently
- **THEN** exactly one run publishes its command and advances the counter, and the other records the budget denial

## ADDED Requirements

### Requirement: The worker process recycles itself under a finite job budget

The browser worker SHALL stop consuming and exit cleanly once it has handled a configured finite
number of jobs per process, counting each job exactly once whether it completed or failed, so its
supervisor starts a fresh Chromium process. The limit SHALL be finite by default and SHALL be
reachable through configuration without code change. A redelivered command deduplicated without a
render SHALL NOT count as a handled job.

#### Scenario: the consumer returns after the configured job count

- **WHEN** a worker configured to handle two jobs receives and finishes two commands
- **THEN** both outcomes are published, both deliveries are acknowledged, the consumer stops pulling, and the process exits successfully

#### Scenario: deduplication does not consume the job budget

- **WHEN** a redelivered already-completed command arrives before the limit is reached
- **THEN** it is acknowledged without a render and the remaining job capacity is unchanged

#### Scenario: failed jobs count toward recycling

- **WHEN** the configured limit is two and one job fails while one completes
- **THEN** the worker exits cleanly after the second terminal outcome

#### Scenario: the deployed binary uses the real renderer without weakening SSRF

- **WHEN** the worker binary starts with a usable Chromium binary
- **THEN** it launches the real Chromium executor, and a loopback render command is rejected as
  `policy_blocked` rather than being treated as browser unavailability
