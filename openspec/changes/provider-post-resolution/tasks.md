## 1. Persistence foundation

- [x] 1.1 Add failing integration test in `crates/eventing/tests/` (`resolution_steps_commit_with_terminal_state`): apply a queued run, call `fail_run` with two resolution steps, and assert the `extractor.provider_resolutions` rows equal the steps in the same transaction as the terminal run state; also assert a second fetch row for the same `run_id` and a second `raw_source` artifact insert succeed. Expected failure today: table absent and terminal functions take no steps parameter (schema/API gap, stated reason).
- [x] 1.2 Edit `schema.sql` in place - drop `fetches.run_id` UNIQUE, drop `artifacts unique (run_id, kind)`, add `extractor.provider_resolutions` (step_id, run_id FK, ordinal, kind check, outcome, failure_class, resolved_url) - and extend `complete_document`, `fail_run`, `reject_quality` to accept `&[ResolutionStep]` inserted in the terminal transaction; make `insert_artifact` a plain insert. Verify: test 1.1 green plus existing eventing and schema-integration suites green.

## 2. Link-post resolution

- [x] 2.1 Add failing end-to-end test `services/extractor/tests/provider_resolution.rs::hn_link_post_completes_from_resolved_article`, following the `pdf_pipeline.rs` harness (extend `test-support` resolver/server registration so the Algolia-origin address string and the article URL are both served by scripted servers; harness-only change, no product code). Assert: run completes with article body text in the document (not discussion text), exactly 2 fetch rows, resolution steps recorded (provider_attempt ok + resolved_target url), and two `raw_source` artifacts. Expected failure today: provider completion ends the run with converted discussion content, one fetch row, no resolution rows (assertion failures).
- [x] 2.2 Implement resolution in `complete_provider`: `AlgoliaItem`/`PostingData` gain the external `url`; provider endpoint mapping moves behind a base-origin function defaulted to the production constants; a non-empty external url (with self-loop guard against the source host's own item pages) triggers full URL-policy validation of the target before any request, then swaps the fetch target and runs one ordinary retrieval/parse/candidate/score pass in the same run, keeping run identity and the provider strategy name while recording the resolved final URL. Verify: test 2.1 green, existing provider and pipeline suites green.

## 3. Unchanged single-request paths

- [x] 3.1 Add characterization tests `hn_ask_post_completes_without_second_request` and `reddit_self_post_completes_without_second_request` in `services/extractor/tests/provider_resolution.rs`: exactly 1 fetch row, no fall-through or resolved-target steps, self-text body becomes the document. Why no failing test first: regression guard for behaviour that must not change; expected green immediately.
- [x] 3.2 Add characterization test `transport_failure_still_terminates_run`: scripted provider connection failure keeps today's terminal `fail_run` behaviour (typed transport class, no HTML fallback). Why no failing test first: pins the D2 boundary that fall-through covers response-content classes only; expected green immediately.

## 4. Fall-through on non-JSON response

- [x] 4.1 Add failing test `non_json_provider_response_falls_through_to_html`: scripted provider reply carries `text/html` media type (challenge-page fixture); assert the run completes from generic extraction of the original normalized URL (second scripted response), with steps showing the typed provider media-type failure plus a successful `html_fallback`, and exactly 2 fetch rows. Expected failure today: run terminates as `provider_response` failure.
- [x] 4.2 Implement the fall-through branch in `complete_provider`: non-JSON media type records the typed failure class and performs exactly one ordinary fetch of the original normalized URL through the standard candidates and evaluator before terminating. Verify: test 4.1 green, suites 3.x still green.

## 5. Fall-through on malformed schema

- [x] 5.1 Add failing test `malformed_provider_schema_falls_through_instead_of_dying`: valid JSON body that violates the item schema (conversion error fixture); same assertions as 4.1 but with the schema-failure class. Expected failure today: run terminates on conversion error.
- [x] 5.2 Route schema-conversion failures into the same single-fallback path as 4.2. Verify: test 5.1 green, suites 3.x-4.x still green.

## 6. Failed fallback terminates with both outcomes

- [x] 6.1 Add failing test `failed_fallback_terminates_recording_both_outcomes`: malformed provider schema AND original URL answering with an unusable page; assert the run terminates with diagnostics naming both the provider failure class and the fallback outcome, and exactly 2 fetch rows. Expected failure today: run dies at the provider step with no fallback attempt recorded.
- [x] 6.2 Record the fallback outcome on termination when the single HTML attempt fails. Verify: test 6.1 green, suites above still green.

## 7. Policy-blocked resolved target

- [x] 7.1 Add failing test `policy_blocked_article_sends_no_second_request`: item url pointing at a prohibited address (metadata/private-range fixture); assert the run terminates with the typed policy failure class recorded on the resolution step and exactly 1 fetch row - no request leaves for the blocked target. Expected failure today: url field ignored entirely (run completes as self-contained content).
- [x] 7.2 Enforce pre-request URL-policy validation of the resolved target and terminal recording of the policy failure. Verify: test 7.1 green, suites above still green.

## 8. Per-host pacing

- [x] 8.1 Add failing tests in `crates/safe-fetch` tests: `requests_to_same_host_are_spaced_by_min_interval` (N rapid requests against a scripted server with interval set; observed inter-request start gaps >= interval) and `pacing_never_extends_operation_deadline` (tight deadline smaller than the required wait yields the existing deadline error class). Expected failure today: admission has no time-based gate, gaps collapse to ~0 and no deadline interaction exists.
- [x] 8.2 Implement the `next_allowed_start` reservation in safe-fetch admission (reservation precedes concurrency-permit acquisition; sleep bounded by remaining deadline), add `FetchConfig.per_host_min_interval_ms: u64` (default 0 disabled) and set a production value in `compose.yaml`. Verify: tests 8.1 green, full workspace suite green with interval unset.

## 9. Documentation and gates

- [x] 9.1 Update README status wording and the `docs/IMPLEMENTATION_PLAN.md` item annotation to describe the resolution half as implemented. Why no failing test first: documentation only.
- [x] 9.2 Run the full gate command list from DEVELOPMENT.md (fmt, clippy, tests, `openspec validate --all`) and confirm exit 0; confirm the fenced command list still matches `.github/workflows/ci.yml`. Why no failing test first: verification step over delivered work.
