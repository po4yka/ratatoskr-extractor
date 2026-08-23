## 1. Crate scaffold and fixture provenance

- [ ] 1.1 Add `crates/providers/tests/fixtures/` with synthetic JSON fixtures modeled on the live API shapes — `hn-story.json` (story with nested comments), `hn-minimal.json`, `reddit-post.json` (t3 listing plus t1 comment listing), `reddit-challenge.html`, and an oversized padded JSON — each written by hand with first-party content, no generator script required. No failing test: static fixtures with first-party provenance.
- [ ] 1.2 Scaffold workspace crate `crates/providers` (`ratatoskr-extractor-providers`, lib `extractor_providers`) depending on contracts, identifiers, document-ir, serde, serde_json, thiserror; wire into workspace, update lockfile. Verify `cargo build -p ratatoskr-extractor-providers --locked` and `cargo deny --locked check` stay green. No failing test: configuration.

## 2. URL mapping and budgets

- [ ] 2.1 Add failing test `crates/providers/tests/mapping.rs::provider_urls_map_to_native_representations`: HN item URLs map to the Algolia endpoint preserving the numeric id, Reddit permalinks map to `.json`, non-comment Reddit URLs and foreign hosts map to None. Run it against a stub returning None for everything and confirm the assertions fail.
- [ ] 2.2 Implement `provider_request` with strict shape matching, make 2.1 pass, run format/Clippy gates, commit this TDD pair.
- [ ] 2.3 Add failing config test `crates/core/tests/config.rs::provider_defaults_are_bounded` asserting new `ProvidersConfig { max_input_bytes, max_blocks }` defaults and override parsing; run it and confirm the section is absent.
- [ ] 2.4 Add `ProvidersConfig` with defaults 8 MiB / 2000 blocks plus intake parser-version mapping extension (`hacker_news`, `reddit` → `providers-v1`) covered by the existing parser-version test's table; make 2.3 pass, run core gates, commit this pair.

## 3. Adapter conversion

- [ ] 3.1 Add failing test `crates/providers/tests/hacker_news.rs::hn_story_becomes_page_ordered_blocks`: stub entry point over `hn-story.json`; assert Heading title block then Paragraphs in pre-order comment order, decoded entities, document title, provenance naming `hacker_news_item` with the source BlobRef, one accepted selected candidate via the shared evaluator, equal digest across two calls. Confirm assertion failure against the stub.
- [ ] 3.2 Implement the Hacker News parser (required identity fields, entity/tag reduction through the shared DOM text extractor, block budget), make 3.1 pass, run gates, commit this pair.
- [ ] 3.3 Add failing test `crates/providers/tests/reddit.rs::reddit_post_and_comments_become_blocks` mirroring 3.1 for `reddit-post.json` with strategy `reddit_post`; run and confirm failure, then implement the Reddit listing parser, make it pass, run gates, commit this pair.

## 4. Typed failure modes

- [ ] 4.1 Add failing tests `crates/providers/tests/failures.rs::{schema_violations_are_typed,oversized_payload_is_resource_limit}`: missing title, wrong listing kinds, and challenge HTML produce typed schema errors; oversized input produces ResourceLimit. Confirm each fails against current success-or-crash behavior.
- [ ] 4.2 Implement typed error mapping (`Schema`, `ResourceLimit`) inside the entry point, make 4.1 pass, run gates, commit this pair.

## 5. Pipeline routing

- [ ] 5.1 Add failing real-PostgreSQL test `services/extractor/tests/pdf_pipeline.rs::claimed_runs_carry_classification`: claim a provider-classified run and read `classification`; run it and confirm the field is absent today.
- [ ] 5.2 Extend `claim_queued_run`'s query and `QueuedRun` with classification, make 5.1 pass, run eventing/service gates, commit this pair.
- [ ] 5.3 Add failing end-to-end tests `pdf_pipeline.rs::hacker_news_run_completes_from_json` (scripted server serving `hn-story.json` at the mapped path for a directly inserted `hacker_news` run: succeeded status, strategy `hacker_news_item`, exactly one request, standard outbox) and `provider_non_json_fails_explicitly` (challenge HTML → class `provider_response`); run both and confirm they fail before routing exists.
- [ ] 5.4 Add the provider branch to `process_run` (map URL, single fetch, adapter parse, typed classes, spawn_blocking, fallback only on unmapped shapes), make 5.3 pass alongside all existing suites, run service/test-support gates, commit this pair.

## 6. Documentation and completion

- [ ] 6.1 Update DEVELOPMENT.md status paragraph and README source-classification list to name the two implemented adapters and their native representations. No test: documentation verified against built behavior.
- [ ] 6.2 Run the exact DEVELOPMENT.md gate order including real PostgreSQL/JetStream tests, the file-size ratchet, `openspec validate --strict`, inspect the diff, archive the change after every gate is green, integrate into `main`, push, and verify remote checks.
