## 1. Golden corpus

- [x] 1.1 RED: add `tools/corpus/tests/golden.rs::golden_corpus_verifies_committed_outputs`, with
  its assertion expecting all named HTML, PDF, Hacker News, Reddit, and YouTube cases to verify;
  run `cargo nextest run --locked -p ratatoskr-extractor-corpus --test golden` and confirm the assertion
  fails because the corpus runner has no matching expectations.
- [x] 1.2 GREEN: add the `ratatoskr-extractor-corpus` tooling crate, repository-owned inputs and canonical
  expected Document IR files for all current block kinds and supported conversion paths; implement
  read-only verification and run `cargo nextest run --locked -p ratatoskr-extractor-corpus --test golden`
  successfully.
- [x] 1.3 RED: add `tools/corpus/tests/bless.rs::bless_updates_only_the_named_case`, asserting that
  a requested case changes only its expected output while ordinary verification writes nothing; run
  `cargo nextest run --locked -p ratatoskr-extractor-corpus --test bless` and confirm its file-content
  assertion fails before the bless action exists.
- [x] 1.4 GREEN: implement the exact-case `corpus-bless` action and its path validation; run the
  bless test and then `cargo run --locked -p ratatoskr-extractor-corpus --bin corpus-bless -- <case>` followed
  by read-only corpus verification.

## 2. Fuzz coverage

- [x] 2.1 Add `fuzz/` cargo-fuzz manifest, structure-aware HTML/PDF/URL target source, and committed
  seed corpora; this is fuzz-tool configuration, so no RED applies. Verify `cargo +nightly fuzz list`
  enumerates exactly the three targets.
- [x] 2.2 Run each seeded target before CI wiring with `cargo +nightly fuzz run <target> --
  -max_total_time=15`; confirm all three complete without a finding or crash artifact.
- [x] 2.3 Add the pinned nightly/cargo-fuzz CI smoke step running all three bounded targets; this is
  CI configuration, so no RED applies. Verify the workflow syntax and the local equivalent command.

## 3. Performance report

- [x] 3.1 RED: add `tools/corpus/tests/performance.rs::baseline_check_rejects_measurements_outside_limits`,
  asserting that a latency, throughput, or 768 MiB memory breach names its metric and limit; run
  `cargo nextest run --locked -p ratatoskr-extractor-corpus --test performance` and confirm the assertion
  fails before report/baseline validation is implemented.
- [x] 3.2 GREEN: implement the offline corpus report, baseline reader/checker, native RSS wrapper,
  and committed baseline JSON; run the performance test and `tools/run-corpus-performance.sh --check`
  successfully.

## 4. Integration and documentation

- [x] 4.1 Update `DEVELOPMENT.md`, `.github/workflows/ci.yml`, `README.md`, and
  `docs/IMPLEMENTATION_PLAN.md` with corpus, bless, fuzz, and report commands/status; this is
  documentation/configuration, so no RED applies. Verify the CI/DEVELOPMENT command-list guard and
  that plan item 9 is checked.
- [x] 4.2 Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D
  warnings`, OpenSpec validation, corpus verification, bounded fuzz smoke, the report check, and the
  full command list in `DEVELOPMENT.md`; record observed results before completion.
