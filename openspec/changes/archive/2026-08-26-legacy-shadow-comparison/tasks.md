## 1. Fixture contract and capture provenance

- [x] 1.1 Add committed, first-party shadow fixtures for web articles, YouTube transcripts, and X posts, each recording a shared sample identity, legacy archive revision/capture provenance, legacy outcome, and class criteria; this is fixture/configuration work, so no RED applies. Verify every fixture is parsed by the new harness test in 2.1.

## 2. Offline comparison and verdicts

- [x] 2.1 RED: add `tools/corpus/tests/shadow.rs::shadow_report_keeps_source_classes_independent`, asserting the committed sample set renders separate web, YouTube, and X sections; web and YouTube can approve on non-inferior results, while an unsupported current X result holds only X. Run `cargo nextest run --locked -p ratatoskr-extractor-corpus --test shadow` and confirm its report assertion fails after the minimal compiling API stub reports no shadow data.
- [x] 2.2 GREEN: add the typed fixture reader and deterministic comparison runner in `tools/corpus`, dispatching the existing current HTML/YouTube corpus paths once per shared sample, representing unsupported current sources explicitly, calculating directional normalized-token overlap and Document IR block statistics, and applying independent committed criteria/verdicts. Make 2.1 pass and run the corpus crate lint/test gate.
- [x] 2.3 RED: extend `tools/corpus/tests/shadow.rs::shadow_report_withholds_approval_for_coverage_regression`, asserting that a below-threshold jointly-successful sample names its overlap and turns only that source class into `hold`; run the focused test and confirm the assertion fails against the initial criteria implementation.
- [x] 2.4 GREEN: implement the coverage-regression diagnostics and stable Markdown report rendering, make 2.3 pass, then run `cargo nextest run --locked -p ratatoskr-extractor-corpus --test shadow`.

## 3. Reviewable report and gate integration

- [x] 3.1 RED: add `tools/corpus/tests/shadow.rs::shadow_report_matches_committed_review_artifact`, asserting read-only verification rejects a changed report and leaves the committed artifact untouched; run it and confirm it fails before the report-verification API exists.
- [x] 3.2 GREEN: add the `shadow-report` binary, committed expected report, and read-only verification API; make 3.1 pass and run the command twice to confirm byte-identical output.
- [x] 3.3 Update `DEVELOPMENT.md`, `README.md`, `.github/workflows/ci.yml`, and `docs/IMPLEMENTATION_PLAN.md` to document the criteria/report command and mark plan item 10 implemented. This is documentation/CI configuration, so no RED applies. Verify the CI/DEVELOPMENT command-list guard stays equal.

## 4. Completion

- [x] 4.1 Run the exact `DEVELOPMENT.md` gate order through `build-gate`, including the offline shadow report verification, then run `openspec validate --strict` and `openspec validate --archived --strict`; inspect the final diff and record all observed outcomes.
- [x] 4.2 Archive the completed OpenSpec change, commit the task branch, integrate it into `main`, push `main`, verify the remote result, and remove only this task worktree and branch after the remote integration is confirmed.
