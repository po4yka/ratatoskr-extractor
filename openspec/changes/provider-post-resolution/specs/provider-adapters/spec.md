# provider-adapters Delta

## MODIFIED Requirements

### Requirement: Provider-classified runs fetch the native representation once

When a claimed run's classification is a supported provider and its normalized URL maps to a
provider-native representation, Extractor SHALL perform exactly one network operation against that
representation through the ordinary safe-fetch path with all SSRF, size, redirect, and timeout
policy applied. Hacker News item URLs SHALL map to their Algolia item endpoint; Reddit comment
permalinks SHALL map to the same permalink with a `.json` suffix. A URL that does not map SHALL
take the ordinary HTML path unchanged. Beyond that single provider operation, a provider-classified
run SHALL perform at most one further network operation: either one retrieval of a resolved
external target, or one generic HTML attempt on the original URL after a provider failure.

#### Scenario: an item URL completes from Algolia JSON

- **WHEN** a queued run classified `hacker_news` claims a news.ycombinator.com item URL
- **THEN** exactly one request hits the mapped Algolia endpoint, the run succeeds with one selected
  candidate, a `document_ir` artifact, the raw JSON kept as `raw_source`, and the standard
  completion events are published

#### Scenario: an unmappable provider URL falls back

- **WHEN** a `reddit`-classified URL does not match the comment-permalink shape
- **THEN** the run fetches the original URL once and processes it as HTML with no second request

### Requirement: Non-JSON and invalid schemas fail explicitly

A provider response whose media type is not `application/json`, or whose body does not satisfy the
adapter schema, SHALL be recorded with a typed provider failure class distinct from generic parse
failures, and the run SHALL then make exactly one ordinary attempt on the original normalized URL
processed as generic HTML before terminating. Error pages and rate-limit challenges SHALL NOT be
misrepresented as extracted articles: the fallback attempt passes the ordinary quality gates like
any other HTML source. When the fallback attempt itself fails, the run SHALL terminate recording
both outcomes.

#### Scenario: an anti-bot page is not mistaken for content

- **WHEN** a provider endpoint answers 200 with an HTML challenge instead of JSON
- **THEN** the run records the provider failure class, makes exactly one generic HTML attempt on
  the original URL, and publishes no document event unless that attempt passes the ordinary
  quality gates

#### Scenario: a malformed schema falls back instead of dying

- **WHEN** a provider response parses as JSON but omits required schema fields
- **THEN** the recorded resolution shows the provider failure class followed by one generic HTML
  attempt on the original URL

#### Scenario: a failed fallback terminates the run

- **WHEN** the generic HTML attempt after a provider failure itself fails retrieval
- **THEN** the run terminates with that retrieval failure class and the persisted resolution
  records both outcomes

## ADDED Requirements

### Requirement: Resolved link posts continue through the ordinary path once

When a provider-native representation identifies external content by URL — a Hacker News item
carrying a story URL or a Reddit link post — Extractor SHALL resolve that canonical target,
validate it under the full URL policy before any request, and continue the run through exactly one
ordinary retrieval and extraction pass on it. The completed document SHALL keep the run's stable
document identity and record both the original capture URL and the resolved final URL. Every
provider attempt outcome, resolved target, and continuation decision SHALL be recorded on the run.
Self-contained posts SHALL keep converting directly from the native representation without a
second network operation.

#### Scenario: a Hacker News link post extracts its article

- **WHEN** a queued run classified `hacker_news` claims an item whose Algolia payload carries an
  external story URL
- **THEN** exactly two requests occur — one to the Algolia endpoint and one to the resolved article
  target — and the completed document keeps the run's document identity with provenance naming the
  adapter strategy and the resolved final URL

#### Scenario: a self-text post converts without a second request

- **WHEN** a `reddit`-classified permalink serves a self-post JSON body
- **THEN** the run succeeds from that single request with no additional network operation

#### Scenario: a resolved target passes policy before any request

- **WHEN** a resolved article target fails URL policy validation
- **THEN** no request is sent to that target and the run terminates with the policy failure class
  after the resolution step is recorded
