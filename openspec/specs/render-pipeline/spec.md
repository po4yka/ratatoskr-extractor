# render-pipeline Specification

## Purpose
Binds this repository to the stored `browser-rendering` contract: the render job schema, the
isolated Chromium worker's execution behaviour, and the extractor's deterministic escalation onto
rendered DOM.

## Requirements

### Requirement: Render job schemas reject everything the contract does not declare

`crates/render-job` SHALL implement the command, completion, and failure payloads with
`deny_unknown_fields` on the command so cookie, token, or storage-state fields cannot be expressed,
SHALL pin evidence as a worker-owned `BlobRef` plus a network summary, and SHALL fix the stable
failure classes (`policy_blocked`, `navigation_timeout`, `total_timeout`, `size_limit`,
`navigation_failed`, `browser_unavailable`). Golden serialization tests SHALL pin the wire shapes.

#### Scenario: a caller cannot smuggle credentials

- **WHEN** a render command carrying a `cookie` field is deserialized
- **THEN** deserialization fails, because the schema declares no such field

#### Scenario: the wire shapes are pinned

- **WHEN** fixture job types serialize
- **THEN** the JSON equals golden bytes reviewed against the stored contract

### Requirement: The browser worker renders one page per fresh context under budgets

The worker deployable SHALL consume durable render commands at least once with KV-backed
`render_id` deduplication, SHALL launch Chromium once per process while opening a fresh context per
job and closing it on every exit path, SHALL deny image, font, media, and WebSocket requests by
default while counting them, SHALL disable downloads, SHALL revalidate the target and every
redirect hop through the shared SSRF policy before navigation proceeds, SHALL enforce navigation,
total, and DOM-size budgets, and SHALL store rendered DOM under its own ownership announcing it by
`BlobRef`.

#### Scenario: a page renders into owned evidence

- **WHEN** a public page inside budgets is requested through a local scripted server
- **THEN** exactly one completion event announces worker-owned bytes whose digest matches, and the
  evidence summary counts every blocked heavy request

#### Scenario: an identical redelivery performs no second render

- **WHEN** the same command arrives after its job completed
- **THEN** the consumer acknowledges it without opening a context

#### Scenario: a redirect toward forbidden space never navigates

- **WHEN** a scripted target redirects toward a policy-forbidden address
- **THEN** the job fails as `policy_blocked`, no further navigation happens, and no completion event
  exists

#### Scenario: oversized DOM produces failure rather than partial evidence

- **WHEN** a rendered document exceeds the size budget
- **THEN** the job fails as `size_limit` and publishes no DOM bytes

### Requirement: Escalation is deterministic, single-shot, and reuses the HTML path

When a direct HTML extraction rejects low-quality content whose raw bytes match empty-shell
evidence (hydration mount markers with near-zero text) and rendering is enabled, Extractor SHALL
publish exactly one render command derived from the run, renew the run lease while awaiting the
result within the render budget, then either re-parse the returned DOM through the ordinary parser,
candidates, evaluator, provenance rules naming the rendered artifact, and standard events, or
terminate the run with the worker's carried failure class. The escalated parse SHALL have no
escalation branch of its own.

#### Scenario: a hydration shell completes from rendered DOM

- **WHEN** the direct fetch returns an empty shell and the escalated render returns hydrated HTML
- **THEN** the run succeeds with provenance naming the rendered artifact, the same evaluator gates
  as direct extraction, exactly one render command, and the expected total request count

#### Scenario: a worker timeout carries through

- **WHEN** the render job fails with `navigation_timeout`
- **THEN** the run terminates with that class and no document event exists
