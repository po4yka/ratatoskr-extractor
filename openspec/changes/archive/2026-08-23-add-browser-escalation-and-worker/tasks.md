## 1. Render job schema

- [x] 1.1 Add failing golden test `crates/render-job/tests/schema.rs::wire_shapes_match_the_contract`: serialize a render command, completion, and failure fixture through the crate types and assert exact JSON (deny_unknown_fields command, BlobRef evidence, stable failure classes); run it against empty stub types and confirm the assertions fail.
- [x] 1.2 Implement the command/event/evidence types per the stored `browser-rendering` contract, make 1.1 pass, run format/Clippy gates, and commit this TDD pair.
- [x] 1.3 Add failing test `::unknown_command_fields_are_rejected`: deserializing a command carrying a `cookie` field fails; run it against the stub and confirm failure, then make 1.1's types enforce it and commit this pair.

## 2. Worker transport and idempotency

- [x] 2.1 Add failing real-NATS test `services/browser-worker/tests/transport.rs::commands_are_consumed_once_and_deduped`: ensure the render stream, publish two identical commands, drive the consumer loop with a stub renderer recording invocations, and assert one invocation plus one KV marker; run it and confirm the stream/consumer/KV wiring is absent today.
- [x] 2.2 Implement stream/consumer setup, the KV dedup bucket, and the consumer loop seam (`RenderExecutor` trait) so the stub compiles, make 2.1 pass, run gates, and commit this pair.

## 3. Chromium rendering

- [x] 3.1 Add failing integration test `services/browser-worker/tests/renderer.rs::renders_a_page_under_budgets` (requires `CHROME_BIN`): serve a scripted page through ScriptedServer, render through the real executor with tight budgets, and assert owned BlobRef bytes contain the page marker, evidence counts blocked image requests, and completion publishes once. Run it and confirm the executor is unimplemented.
- [x] 3.2 Implement the Chromium executor (launch-once, fresh context, CDP interception, budgets, teardown in all paths) and make 3.1 pass, run gates, and commit this pair.
- [x] 3.3 Add failing tests `::policy_blocked_redirects_refuse_navigation` and `::size_limit_fails_without_partial_dom`: a scripted redirect to a forbidden address fails as `policy_blocked`; a page exceeding the byte cap fails as `size_limit` with no completion event. Run and confirm, implement, make pass, and commit this pair.

## 4. Extractor escalation

- [x] 4.1 Add failing real-PostgreSQL/NATS test `services/extractor/tests/pdf_pipeline.rs::empty_shell_escalates_and_completes_from_rendered_dom`: a scripted server returns an empty hydration shell for the direct fetch and the rendered page for the worker's fetch; with `render.enabled` the run completes with provenance naming the rendered BlobRef and exactly three total requests. Run it and confirm escalation is absent today.
- [x] 4.2 Implement the escalation path (shell detection after quality rejection, one render command, lease renewal while awaiting, re-parse through `from_html`, terminal mapping of carried failure classes, no second escalation), make 4.1 pass alongside every existing suite, run service gates, and commit this pair.
- [x] 4.3 Add failing test `::worker_failure_classes_reach_terminal_state`: a render command answered by `evt.content.render.failed.v1` with class `navigation_timeout` terminates the run with that class. Run, confirm, implement the mapping remainder, and commit this pair.

## 5. Deployment and documentation

- [x] 5.1 Add `deploy/browser-worker/Dockerfile` (Debian + Chromium + the compiled binary) and a profiled `browser` compose service with memory/PID limits; verify with `docker compose config -q`. No failing test: deployment configuration; syntax validation is the check.
- [x] 5.2 Document the `CHROME_BIN` prerequisite beside the PostgreSQL/JetStream ones in `DEVELOPMENT.md`, add the Chrome installation step to `.github/workflows/ci.yml` so the one-list invariant holds, and update README status lines. No test: documentation verified against the built binaries.
- [x] 5.3 Run the exact DEVELOPMENT.md gate order including real PostgreSQL/JetStream/Chrome tests, the file-size ratchet, and `openspec validate --strict`; archive the change after every gate is green, integrate into `main`, push, and verify remote checks.
