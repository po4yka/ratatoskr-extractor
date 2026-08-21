## Purpose

Defines deterministic URL identity, source ownership routing, and the mandatory network policy applied before any connection is attempted.

## ADDED Requirements

### Requirement: Normalization preserves the request and produces a stable key
The extractor SHALL retain the original URL separately and derive a deterministic normalized URL by lowercasing the scheme and host, removing the fragment and default port, removing only the documented tracking parameters, and retaining other query parameters without changing their order.

#### Scenario: Equivalent tracked URLs share one normalized form
- **WHEN** two HTTP URLs differ only by scheme or host casing, a default port, a fragment, and documented tracking parameters
- **THEN** their normalized URLs and routing fingerprints are equal while each original URL remains unchanged

### Requirement: Ambiguous and unsupported URLs are refused
The extractor SHALL accept only absolute HTTP and HTTPS URLs with a host, no user information, an allowed port, and a bounded serialized length. It SHALL reject unsupported schemes and ambiguous address forms before DNS resolution.

#### Scenario: User information is rejected before resolution
- **WHEN** a URL contains a username or password before an otherwise public host
- **THEN** normalization returns an invalid-URL error and no resolver is called

### Requirement: Known source ownership is classified before generic web
The extractor SHALL classify exact hosts and their documented subdomains into delegated provider routes, public source-adapter routes, direct PDF candidates, or generic web. A lookalike suffix SHALL remain generic web.

#### Scenario: A provider lookalike is not delegated
- **WHEN** the host is `github.com.example.test` rather than `github.com` or its documented subdomain
- **THEN** classification returns generic web and not the GitHub-owned route

### Requirement: Prohibited addresses never become connection targets
The network policy SHALL deny unspecified, loopback, private, shared, link-local, documentation, benchmarking, multicast, reserved, metadata-service, IPv4-mapped prohibited IPv6, and other non-global addresses. It SHALL apply to every address returned for a host.

#### Scenario: Mixed DNS answers are denied as one target
- **WHEN** a hostname resolves to one public address and one prohibited address
- **THEN** the destination is policy-denied and no address from that answer is used

### Requirement: Resolution failures and policy denials remain distinct
The extractor SHALL distinguish an empty or failed DNS answer from a successful answer that policy refuses. User-facing diagnostics SHALL NOT reveal the prohibited address.

#### Scenario: A prohibited result is not reported as DNS failure
- **WHEN** DNS succeeds with only loopback or private addresses
- **THEN** the result is a policy denial with a bounded reason class and not a DNS failure containing address text

### Requirement: Policy is repeated for every resolution
The extractor SHALL apply the same address policy whenever a hostname is resolved, including after a redirect and after a DNS answer changes. A prior safe answer SHALL NOT authorize a later prohibited answer.

#### Scenario: A changed DNS answer is revalidated
- **WHEN** the first resolution is public and a later resolution for the same host is prohibited
- **THEN** the later connection attempt is denied before transport use
