## Context

See `proposal.md` for motivation. The repository contains documents only. `ratatoskr-platform` is the deployed reference for workspace policy, typed configuration, telemetry, health, test process control, CI, and systemd hardening. The cross-repository change `unblock-the-first-domain-service` already fixes two boundaries: bytes remain in extractor-owned storage and only `BlobRef` crosses the boundary; Document IR starts at plan item 4.

The ordinary path must fetch once, validate every resolution and redirect, keep memory bounded, and preserve evidence. The development rules forbid migrations and later major versions. The user also requires all production paths to avoid `unwrap`, `expect`, and panic.

## Goals / Non-Goals

**Goals:**

- Establish a small independently buildable Rust service with the same operational shape as Platform.
- Make URL and network safety mandatory properties of the public library API.
- Stream one response into one immutable raw artifact while calculating its reference and metadata.
- Leave deterministic seams for DNS, time, jitter, transport, and local HTTP tests.

**Non-Goals:**

- No candidate extraction, quality scoring, PDF/provider adapter, or browser process.
- No authenticated requests, ambient proxy, cookies, provider session, or local-file input.
- No blob HTTP service, signed blob URL, or off-host replication.

## Decisions

### 1. Use seven narrow workspace members

The workspace will contain `crates/core`, `crates/telemetry`, `crates/url-routing`, `crates/blob-store`, `crates/safe-fetch`, `crates/test-support`, and `services/extractor`.

- `core` owns the typed config and startup error vocabulary.
- `telemetry` owns the one subscriber and Prometheus recorder.
- `url-routing` owns normalization, classification, address policy, and the resolver seam.
- `blob-store` owns extractor-local content-addressed files and `BlobRef` construction and verification.
- `safe-fetch` owns the shared HTTP client, admission, redirects, retry, decoding, metadata, and orchestration into `blob-store`.
- `test-support` owns local servers, scripted DNS, clocks, and temporary blob roots and is never linked into the service.
- `services/extractor` owns process startup, admin routes, health state, signals, and shutdown.

The dependency direction is leaf-first in that order. There is no `Fetcher` or `BlobStore` trait with one production implementation. Tests inject only the small effects that must vary: resolver answers, time/jitter, and the HTTP transport boundary. A separate HTTP crate is deferred because only one service owns four admin routes.

Alternative: one crate. Rejected because test-only network controls and process-global telemetry would then enter the same dependency surface as deterministic URL policy. Alternative: reproduce the full Platform crate graph. Rejected because persistence, eventing, identity, and public API do not exist in this slice.

### 2. Copy Platform's workspace and process policy, not its unrelated domains

The root manifest, Rust 1.97 toolchain, edition, rustfmt settings, lint levels, Clippy thresholds, `cargo-deny` policy, release profile, CI command order, config loader shape, telemetry bootstrap, health response shape, and systemd hardening start from `../platform`. Every member inherits workspace lints. Production code forbids unsafe code, panic, unwrap, expect, unchecked indexing, and string slicing.

The config has `admin`, `blobs`, `fetch`, `shutdown`, and `telemetry` sections. It uses built-in defaults followed by `RATATOSKR__` environment variables with `__` nesting. The blob root is required because there is no safe durable default that works both on a laptop and on the target. The systemd environment sets `/mnt/nvme/ratatoskr/blobs/ratatoskr-extractor`. Fetch defaults allow ports 80 and 443, disable ambient proxies, and give every budget a finite value. A test-only policy may admit a loopback listener; no runtime flag can do so.

Alternative: a configuration file. Rejected for the same reason as Platform: one source mechanism is enough until an operator needs another. Alternative: default the blob root to a relative directory. Rejected because a service working directory is not durable ownership.

### 3. Normalize conservatively and classify exact host boundaries

The `url` crate parses and serializes addresses. Normalization removes fragments, default ports, and a closed tracking-parameter list (`utm_source`, `utm_medium`, `utm_campaign`, `utm_term`, `utm_content`, `gclid`, and `fbclid`). It preserves the order and spelling of all other query pairs. The original parsed URL and normalized URL remain separate. SHA-256 of the normalized serialized URL is the routing fingerprint.

Classification uses exact host or dot-delimited subdomain matches. GitHub and social hosts return delegated routes. Reddit, Hacker News, and YouTube return future public-adapter routes. A path ending in `.pdf` after case folding is a PDF candidate; MIME evidence can correct it after fetch. Everything else is generic web. Classification does not fetch.

Alternative: sort all query parameters. Rejected because order can be semantically meaningful. Alternative: use page canonical metadata now. Rejected because no page exists before fetch and a page claim is untrusted evidence.

### 4. Apply one closed network policy at every DNS use

Validation rejects non-HTTP schemes, missing hosts, user information, overlong URLs, port zero, and ports outside the configured allowlist. Address classification uses standard-library address inspection plus explicit ranges where the standard library does not express the product rule. IPv4-mapped IPv6 is converted and checked as IPv4. A DNS answer is accepted only when it is non-empty and every address is globally routable under the policy.

The production Reqwest client receives a custom resolver that applies the same policy each time it resolves. The redirect loop also normalizes and validates the next URL before request construction. Resolver results carry typed DNS or policy errors so transport errors do not erase the distinction. Error rendering exposes only a closed reason code.

Alternative: resolve once before calling Reqwest. Rejected because the HTTP stack could resolve again and connect to a different answer. Alternative: accept the public subset of a mixed answer. Rejected because an attacker can control address selection and rebinding.

### 5. Use one shared Reqwest/Rustls client with redirects, cookies, and proxies disabled

The service constructs one client for the lifetime of its policy. Automatic redirects are off. Cookies are off. Ambient proxy discovery is off. The client carries the explicit user agent, pool idle limits, connect cap, and the validating resolver. Manual redirect handling joins relative `Location` values, strips fragments, validates the next hop, and stores bounded history.

The operation creates one monotonic deadline. Every DNS, connect, header, retry delay, redirect, body read, decode, and store step gets the smaller of its phase cap and remaining time. Retry is limited to idempotent GET, a replayable empty body, explicit transient classes, and a small total-attempt cap. `Retry-After` and full jitter never sleep beyond the original deadline.

Alternative: one client per request with pinned addresses. Rejected because it discards pooling. Alternative: Reqwest automatic redirects and retries. Rejected because neither boundary can enforce and record all Ratatoskr hop rules.

### 6. Count wire and decoded bytes and store decoded source bytes

Transparent Reqwest decompression is disabled. The fetcher requests supported encodings and wraps the response byte stream in a counting reader. A bounded decoder handles each supported `Content-Encoding`; another counter bounds the decoded stream. `Content-Length` rejects an excessive declared wire length before streaming, but the actual wire counter remains authoritative. The stored raw artifact is the decoded representation that the next parser consumes. Metadata retains the encoding and both counts.

The fetcher buffers only the small sniff prefix needed to compare declared and effective media type. HTML and PDF signatures are detected directly; unknown bytes remain `application/octet-stream`. No general file-type framework is added in this slice.

Alternative: ask only for `identity`. Rejected because a server can ignore the request and because it would leave the required decompression-bomb boundary untested. Alternative: store the compressed transport bytes. Rejected because every parser would then need a second decoding path and a second resource policy.

### 7. Commit artifacts atomically under the owning service root

Bytes stream into a uniquely named file in `<root>/.staging/` while SHA-256 and decoded length are calculated. On success, the store creates `<root>/sha256/<first-two-hex>/<remaining-hex>`, flushes the file, and atomically renames the staging file. An existing target is verified by digest and length and reused. Cancellation and error paths remove the staging file; startup also removes stale staging files left by process death.

The returned contract type is `ratatoskr_identifiers::BlobRef` from the SHA-pinned contracts repository. It contains owner `ratatoskr-extractor`, SHA-256, the effective media type, and stored byte length. Path resolution remains an internal function that validates owner and digest before opening a file.

Alternative: store URLs or absolute paths in events. Rejected by the workspace blob-reference spec. Alternative: create a storage service. Rejected by the accepted cross-repository design and by the one-host deployment.

### 8. Treat cache validation as evidence tied to a verified artifact

`FetchRequest` can carry a prior cache record with optional ETag and Last-Modified plus its `BlobRef`. The fetcher sends only those conditional headers. On `304`, it verifies the referenced bytes before returning `Revalidated`; a missing or mismatched artifact is `CacheIntegrity`. A non-304 response creates a new artifact and cache metadata. Challenge and failed partial responses never replace the prior record.

Alternative: treat every 304 as success. Rejected because a validator without its body is not content. Alternative: build a cache database now. Rejected because plan item 6 owns persistence and no schema is needed for the pure contract.

### 9. Keep the process useful without inventing an inbound API

The binary exposes only `/health/live`, `/health/ready`, `/metrics`, and `/version`. It validates the blob root, builds the fetch stack, binds the admin listener, marks readiness, and waits for termination. The safe-fetch crate is exercised through integration tests until plan item 6 adds the command consumer. A temporary public fetch endpoint would become an unsupported product API and is not added.

The systemd unit follows Platform's service identity, restart, logging, resource, filesystem, address-family, capability, and syscall rules. It grants write access only to the extractor blob tree. It does not copy Platform's `IPAddressDeny=any`, because public HTTP egress is this service's job; SSRF policy remains mandatory in the process.

### 10. Parse one bounded HTML DOM into the shared contract

`document-ir` reads verified extractor-owned bytes, parses them once with `html5ever` into a
small service-owned tree, and walks that one DOM to produce the version-one shared block
intersection: headings and
paragraphs. It normalizes only whitespace, preserves reading order, records one provenance entry per
block, and hashes the contracts repository's canonical JSON rendering of `blocks`. Node and input
budgets are mandatory. Parsing runs in `spawn_blocking`; active HTML is never rendered or executed.

This is not plan item 5: no competing candidates, score, selector strategy, or acceptance threshold
exists. The fixed strategy is `html_primitives`, sufficient to create the shared primitive and to
exercise persistence and event delivery.

### 11. Copy Platform's durable delivery pattern into the owned schema

One editable root `schema.sql` creates only `extractor.*`: sources, fetches, artifacts,
extraction_runs, candidates, inbox_events, and outbox_events. There are no migration files or
migration tool. The process owns one finite SQLx pool. A command transaction inserts the inbox row
and a queued run together; duplicate `command_id` inserts nothing. Network and parsing work happens
outside a database transaction. Completion writes owned records and both outgoing envelopes into the
outbox in one transaction.

The outbox claimant uses a finite lease, bounded exponential retry, `FOR UPDATE SKIP LOCKED`, a
maximum attempt count, and JetStream acknowledgement before marking a row published. The command
consumer uses a durable pull consumer and acknowledges only after the inbox/run transaction commits.
The service owns and joins the consumer, worker, and publisher loops through one cancellation tree.

Platform's command envelope is not yet a published contracts type. Extractor therefore deserializes
the exact wire members Platform currently emits and preserves additive-envelope compatibility, while
rejecting the wrong subject/type, invalid identifiers, or a non-HTTP payload before any state change. Outgoing Document IR and
`OperationReported` use the SHA-pinned published contract types. The
`content.document.extracted.v1` payload is exactly the published `Document` object, with no local
cross-repository fields.

## Risks / Trade-offs

- [A custom DNS resolver can lose the original error class through the HTTP library] -> Add a resolver-level typed test and map only known wrapper causes; keep policy validation at redirect construction as a second boundary.
- [Manual decompression adds parser and dependency surface] -> Support only the encodings advertised by the client, cap both representations, and run `cargo deny` plus malformed-stream tests.
- [Atomic rename durability varies by filesystem] -> Keep staging and target on the same configured root, flush before rename, and test the target deployment filesystem during rollout.
- [Default ports 80 and 443 exclude legitimate high-port sites] -> Keep an explicit bounded allowlist in typed config; expansion is an operator decision and never disables address checks.
- [No event consumer means the running service fetches nothing yet] -> This is deliberate sequencing from the user: item 6 adds the bus. Library integration tests prove items 2 and 3 without publishing a temporary API.
- [The systemd network filter cannot express public-only egress while also admitting admin scrapes cleanly] -> Do not claim that unit hardening enforces SSRF; enforce it in the resolver and validate host firewall policy separately at deployment.

## Migration Plan

1. Build and validate the workspace locally and in CI.
2. Install the binary, environment file, service user, log path, and extractor-owned blob directory on the target.
3. Run `check-config`, then start the unit and verify liveness, readiness, metrics, version, log output, resource ceilings, and blob-root permissions.
4. No capture traffic is routed to it until plan item 6 adds the command consumer.

Rollback stops and removes the unit and binary. Raw artifacts are immutable and can remain for inspection or be removed later under an explicit retention decision. There is no database or migration to reverse.
