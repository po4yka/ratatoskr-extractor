# Safe fetch

## Purpose

Defines one bounded retrieval path that preserves response evidence and stores immutable raw source bytes under an extractor-owned content address.

## Requirements

### Requirement: Retrieval uses one bounded operation budget
Every fetch SHALL use one absolute operation deadline across DNS, connection, response headers, redirects, retries, retry delays, decompression, and artifact storage. Connect, first-byte, read-idle, and total deadline failures SHALL remain distinct.

#### Scenario: Redirects do not restart the deadline
- **WHEN** a response redirects after consuming most of the operation allowance
- **THEN** the next hop receives only the remaining allowance and the operation ends by the original deadline

### Requirement: Redirects are explicit and revalidated
The extractor SHALL disable automatic redirects, resolve relative locations against the current URL, normalize and validate every next target, and stop at the configured hop limit. It SHALL NOT forward authorization, cookies, or other ambient credentials.

#### Scenario: Redirect to a prohibited address is blocked
- **WHEN** an allowed server returns a redirect whose target resolves to a prohibited address
- **THEN** the extractor returns a redirect policy denial without sending a request to that target

### Requirement: Response bytes are streamed under hard limits
The extractor SHALL reject an excessive declared length before body allocation, count actual wire and decoded bytes while streaming, stop when either configured limit is exceeded, and never require the complete body in memory. Supported content encodings SHALL be decoded under the same absolute deadline; unsupported encodings SHALL fail explicitly.

#### Scenario: A decompression expansion is stopped
- **WHEN** a response is within the wire-byte limit but its decoded representation exceeds the decoded-byte limit
- **THEN** retrieval stops with a decoded-body-limit error and no committed artifact exists

### Requirement: Retries are bounded and safe
The extractor SHALL retry only a replayable idempotent retrieval after an eligible transient DNS, connect, or response result. It SHALL use a finite attempt count, full jitter, valid `Retry-After`, and the original operation deadline. It SHALL NOT retry policy, TLS identity, unsupported-format, body-limit, or deterministic artifact failures.

#### Scenario: A policy denial is attempted once
- **WHEN** destination validation returns a policy denial and the retry limit is greater than one
- **THEN** the operation returns that denial with an attempt count of one and no retry delay

### Requirement: Fetch metadata preserves cache and provenance evidence
The result SHALL record the original, normalized, and final URLs; redirect history; status; declared and effective media type evidence; allowed response headers; wire and decoded byte counts; content digest; timings; attempt count; and cache outcome. It SHALL expose `ETag` and `Last-Modified` as conditional request metadata without recording cookies, authorization, or secret query values.

#### Scenario: Validators are returned without sensitive headers
- **WHEN** a response includes `ETag`, `Last-Modified`, `Set-Cookie`, and an authorization challenge
- **THEN** fetch metadata contains the two validators and excludes cookie and authorization values

### Requirement: A 304 response resolves only to verified prior bytes
Conditional retrieval SHALL accept `304 Not Modified` only when the caller supplies a prior `BlobRef` whose bytes exist and match its digest and length. A missing or mismatched prior artifact SHALL fail rather than create a bodyless success.

#### Scenario: Missing cached bytes invalidate a 304
- **WHEN** the server returns `304` and the referenced prior artifact is missing
- **THEN** the fetch returns a cache-integrity error and does not report a successful artifact

### Requirement: Raw artifacts are immutable and content addressed
Successful decoded source bytes SHALL be committed atomically beneath the extractor's own configured root using their SHA-256 digest, and the result SHALL announce them with a shared `BlobRef` carrying owner `ratatoskr-extractor`, digest, effective media type, and exact stored length. Filesystem paths SHALL NOT cross the repository boundary.

#### Scenario: Stored artifact matches its reference
- **WHEN** a response body is stored successfully
- **THEN** resolving the returned reference reads the same bytes, their SHA-256 digest and length match the reference, and no URL or filesystem path appears in the reference

### Requirement: Failed writes leave no published artifact
A cancellation, timeout, stream error, limit failure, or storage failure SHALL leave no committed content-addressed file and SHALL NOT return a `BlobRef`. Incomplete staging files SHALL be removable without interpreting source-controlled names.

#### Scenario: A failed stream cannot be announced
- **WHEN** the response stream fails after at least one chunk was written
- **THEN** the fetch returns the stream error, no `BlobRef` is returned, and no committed digest path exists

### Requirement: Network work is bounded by admission controls
The extractor SHALL enforce finite global and per-host in-flight fetch limits before DNS or body work begins. Refused work SHALL not enter an unbounded queue.

#### Scenario: Per-host capacity refuses excess work
- **WHEN** the configured per-host capacity is occupied and another fetch for that host is submitted
- **THEN** the new fetch is refused with an overload error before its resolver or transport is used
