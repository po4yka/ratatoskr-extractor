## 1. Candidate generation

- [x] 1.1 Add failing test `crates/document-ir/tests/candidates.rs::semantic_article_beats_page_chrome`; assert noisy navigation and footer blocks are absent and selected provenance is `semantic`. Run it and confirm the current `html_primitives` document contains page chrome.
- [x] 1.2 Add semantic, readability-compatible, and density extraction over the existing DOM, return candidate decisions with the selected Document, make 1.1 pass, run document-ir format and Clippy gates, and commit this TDD pair on `main`.

## 2. Deterministic quality

- [ ] 2.1 Add failing test `crates/document-ir/tests/quality.rs::evaluation_is_repeatable_with_stable_ties`; evaluate one candidate set repeatedly and assert exact component totals, reasons, acceptance, and semantic-first tie selection. Run it and confirm the scaffold decisions differ from the expected fixed-point values.
- [ ] 2.2 Implement the fixed-point component formula, acceptance thresholds, evaluator version, bounded reasons, and stable tie order; make 2.1 pass, run document-ir format and Clippy gates, and commit this TDD pair on `main`.
- [ ] 2.3 Add failing test `crates/document-ir/tests/quality.rs::login_shell_is_rejected`; assert a short login and consent page returns the bounded low-quality error rather than Document IR. Run it and confirm the current conversion succeeds.
- [ ] 2.4 Refuse extraction when no candidate meets both thresholds, make 2.3 pass, run document-ir format and Clippy gates, and commit this TDD pair on `main`.

## 3. Durable candidate decisions

- [ ] 3.1 Add failing real-PostgreSQL test `crates/persistence/tests/records.rs::one_selected_candidate_is_enforced_per_run`; assert `selected` is stored, one run cannot select two candidates, and idempotent rewrites keep one selected row. Run it and confirm the column or constraint is absent.
- [ ] 3.2 Add the selected field and partial unique index to editable `schema.sql`, update `CandidateRecord` without migration tooling, make 3.1 pass, run persistence format and Clippy gates, and commit this TDD pair on `main`.
- [ ] 3.3 Add failing real pipeline tests `services/extractor/tests/command_pipeline.rs::completed_html_persists_all_candidate_decisions_atomically` and `quality_rejection_persists_evidence_without_document_event`; assert success commits three scored candidates, exactly one selected strategy matching emitted Document provenance, artifacts, terminal state, and two events together, while rejection commits three unselected candidates, raw evidence, terminal failure, and one failed report without Document IR or document event. Run them and confirm completion omits candidate rows and no atomic rejection path exists.
- [ ] 3.4 Extend the existing terminal transaction to record every candidate decision with success or bounded quality rejection, make 3.3 pass, run service, eventing, and persistence format/test/Clippy gates, and commit this TDD pair on `main`.

## 4. Calibration and completion

- [ ] 4.1 Add failing corpus test `crates/document-ir/tests/corpus.rs::calibration_cases_keep_expected_winners_and_score_ranges` plus four minimized synthetic fixtures for semantic, noisy, malformed, and login HTML; assert each winner or rejection and score range. Run it and confirm at least one expectation fails before calibration.
- [ ] 4.2 Calibrate only the shared weights and thresholds needed to make 4.1 pass, rerun all document-ir tests and review every fixture, then commit this TDD pair on `main`.
- [ ] 4.3 Mark only Extractor implementation-plan item 5 complete and update current README/testing text. No test: documentation; verify names, thresholds, fetch count, parse count, and deferred item 9 against the built behavior, then commit the documentation atomically on `main`.
- [ ] 4.4 Run the exact `DEVELOPMENT.md` gate order, all real PostgreSQL and JetStream tests, `openspec validate select-deterministic-html-candidates --strict`, source-length and forbidden-panic audits, and inspect the final diff. Push `main` only after every gate is green and verify the remote SHA and GitHub Actions.
