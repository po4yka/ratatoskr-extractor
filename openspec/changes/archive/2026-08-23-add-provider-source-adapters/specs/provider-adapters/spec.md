## Purpose

Defines deterministic conversion of provider-native JSON into shared Document IR for classified
Hacker News and Reddit URLs, including URL mapping, bounded schemas, explicit failure modes, and
single-fetch semantics.

## ADDED Requirements

### Requirement: Provider-classified runs fetch the native representation once

When a claimed run's classification is a supported provider and its normalized URL maps to a
provider-native representation, Extractor SHALL perform exactly one network operation against that
representation through the ordinary safe-fetch path with all SSRF, size, redirect, and timeout
policy applied. Hacker News item URLs SHALL map to their Algolia item endpoint; Reddit comment
permalinks SHALL map to the same permalink with a `.json` suffix. A URL that does not map SHALL
take the ordinary HTML path unchanged.

#### Scenario: an item URL completes from Algolia JSON

- **WHEN** a queued run classified `hacker_news` claims a news.ycombinator.com item URL
- **THEN** exactly one request hits the mapped Algolia endpoint, the run succeeds with one selected
  candidate, a `document_ir` artifact, the raw JSON kept as `raw_source`, and the standard
  completion events are published

#### Scenario: an unmappable provider URL falls back

- **WHEN** a `reddit`-classified URL does not match the comment-permalink shape
- **THEN** the run fetches the original URL once and processes it as HTML with no second request

### Requirement: Adapter conversion is bounded and deterministic

Provider adapters SHALL parse tolerant JSON against required schemas with finite budgets on source
bytes and produced blocks, SHALL order story text before comments in API order, and SHALL produce
identical blocks, provenance, and content digest for identical input bytes. Provenance SHALL name
the adapter strategy (`hacker_news_item`, `reddit_post`) and reference the raw artifact.

#### Scenario: identical payloads hash identically

- **WHEN** the same fixture payload is converted twice
- **THEN** both results have equal ordered blocks, equal content digest, and equal candidate
  decisions

#### Scenario: a budget is exceeded

- **WHEN** a payload exceeds the configured input-byte or block budget
- **THEN** conversion fails with a typed resource-limit error before any Document IR is built

### Requirement: Non-JSON and invalid schemas fail explicitly

A provider response whose media type is not `application/json`, or whose body does not satisfy the
adapter schema, SHALL terminate the run with a typed provider failure class distinct from generic
parse failures. Error pages and rate-limit challenges SHALL NOT be misrepresented as extracted
articles.

#### Scenario: an anti-bot page is not mistaken for content

- **WHEN** a provider endpoint answers 200 with an HTML challenge instead of JSON
- **THEN** the run terminates with the provider failure class and publishes no document event
