-- Current extractor-owned PostgreSQL schema. Development has no migration history: a schema
-- change edits this file and test databases are created from it.

create schema extractor;

create table extractor.sources (
    source_id       uuid        primary key,
    owner_id        uuid        not null,
    original_url    text        not null,
    normalized_url  text        not null,
    canonical_url   text        not null,
    host             text        not null,
    classification   text        not null,
    created_at       timestamptz not null,
    constraint sources_urls_are_bounded check (length(original_url) between 1 and 8192 and length(normalized_url) between 1 and 8192 and length(canonical_url) between 1 and 8192),
    constraint sources_host_is_bounded check (length(host) between 1 and 253),
    unique (owner_id, normalized_url)
);

create table extractor.inbox_events (
    command_id    uuid        primary key,
    subject       text        not null,
    command_type  text        not null,
    producer      text        not null,
    received_at   timestamptz not null,
    applied_at    timestamptz,
    outcome       text,
    constraint inbox_subject_is_capture_requested check (subject = 'cmd.content.capture.requested.v1'),
    constraint inbox_command_type_is_capture_requested check (command_type = 'content.capture.requested.v1'),
    constraint inbox_producer_is_bounded check (length(producer) between 1 and 64),
    constraint inbox_outcome_matches_completion check ((outcome is not null) = (applied_at is not null)),
    constraint inbox_outcome_is_known check (outcome is null or outcome in ('applied', 'duplicate', 'rejected'))
);

create table extractor.extraction_runs (
    run_id              uuid        primary key,
    command_id          uuid        not null unique references extractor.inbox_events (command_id),
    operation_id        uuid        not null,
    owner_id            uuid        not null,
    correlation_id      text        not null,
    source_id           uuid        not null references extractor.sources (source_id),
    status              text        not null,
    policy_version      text        not null,
    normalizer_version  text        not null,
    parser_version      text        not null,
    document_id         uuid        not null unique,
    queued_at           timestamptz not null,
    started_at          timestamptz,
    completed_at        timestamptz,
    last_error_class    text,
    claimed_until       timestamptz,
    claimed_by          text,
    constraint extraction_run_status_is_known check (status in ('queued', 'running', 'succeeded', 'failed')),
    constraint extraction_run_times_match_status check ((started_at is not null) = (status in ('running', 'succeeded', 'failed')) and (completed_at is not null) = (status in ('succeeded', 'failed'))),
    constraint extraction_run_error_is_safe check (last_error_class is null or length(last_error_class) between 1 and 64),
    constraint extraction_run_claim_is_whole check ((claimed_until is null) = (claimed_by is null)),
    constraint extraction_run_claimed_by_is_bounded check (claimed_by is null or length(claimed_by) between 1 and 64)
);

create index extraction_runs_queued_idx on extractor.extraction_runs (queued_at) where status in ('queued', 'running');

create table extractor.fetches (
    fetch_id             uuid        primary key,
    run_id               uuid        not null references extractor.extraction_runs (run_id),
    final_url            text        not null,
    http_status          integer     not null,
    media_type           text        not null,
    wire_bytes           bigint      not null,
    decoded_bytes        bigint      not null,
    attempts             integer     not null,
    cache_outcome        text        not null,
    etag                 text,
    last_modified        text,
    fetched_at           timestamptz not null,
    constraint fetch_status_is_http check (http_status between 100 and 599),
    constraint fetch_sizes_are_non_negative check (wire_bytes >= 0 and decoded_bytes >= 0),
    constraint fetch_attempts_are_positive check (attempts > 0),
    constraint fetch_cache_outcome_is_known check (cache_outcome in ('fresh', 'revalidated'))
);

create table extractor.artifacts (
    artifact_id      uuid        primary key,
    run_id           uuid        not null references extractor.extraction_runs (run_id),
    kind             text        not null,
    owner_service    text        not null,
    digest_algorithm text        not null,
    digest_hex       text        not null,
    media_type       text        not null,
    length_bytes     bigint      not null,
    created_at       timestamptz not null,
    constraint artifact_kind_is_known check (kind in ('raw_source', 'document_ir', 'diagnostics', 'archived_media')),
    constraint artifact_is_extractor_owned check (owner_service = 'ratatoskr-extractor'),
    constraint artifact_digest_is_sha256 check (digest_algorithm = 'sha256' and digest_hex ~ '^[0-9a-f]{64}$'),
    constraint artifact_length_is_non_negative check (length_bytes >= 0)
);

create table extractor.media_archives (
    media_id      uuid        primary key,
    run_id        uuid        not null references extractor.extraction_runs (run_id),
    video_id      text        not null,
    digest_hex    text        not null,
    media_type    text        not null,
    length_bytes  bigint      not null,
    created_at    timestamptz not null,
    expires_at    timestamptz not null,
    constraint media_archives_video_id_is_bounded check (length(video_id) between 1 and 32),
    constraint media_archives_digest_is_sha256 check (digest_hex ~ '^[0-9a-f]{64}$'),
    constraint media_archives_length_is_positive check (length_bytes > 0),
    constraint media_archives_expiry_follows_creation check (expires_at > created_at)
);

create index media_archives_expiry_idx on extractor.media_archives (expires_at);

create table extractor.candidates (
    candidate_id      uuid        primary key,
    run_id            uuid        not null references extractor.extraction_runs (run_id),
    strategy          text        not null,
    extractor_version text        not null,
    metrics           jsonb       not null,
    score             double precision,
    reasons           jsonb       not null,
    selected          boolean     not null,
    artifact_id       uuid        references extractor.artifacts (artifact_id),
    created_at        timestamptz not null,
    constraint candidate_strategy_is_bounded check (length(strategy) between 1 and 64),
    constraint candidate_metrics_are_object check (jsonb_typeof(metrics) = 'object'),
    constraint candidate_reasons_are_array check (jsonb_typeof(reasons) = 'array'),
    constraint candidate_score_is_unit_interval check (score is null or score between 0.0 and 1.0),
    unique (run_id, strategy, extractor_version)
);

create unique index candidates_one_selected_per_run_idx on extractor.candidates (run_id) where selected;

create table extractor.provider_resolutions (
    step_id        uuid        primary key,
    run_id         uuid        not null references extractor.extraction_runs (run_id),
    ordinal        integer     not null,
    kind           text        not null,
    outcome        text,
    failure_class  text,
    resolved_url   text,
    created_at     timestamptz not null default now(),
    constraint provider_resolution_kind_is_known check (kind in ('provider_attempt', 'resolved_target', 'html_fallback', 'render_policy'))
);

create table extractor.outbox_events (
    outbox_id             uuid        primary key,
    message_id            uuid        not null unique,
    causation_command_id  uuid        not null references extractor.inbox_events (command_id),
    operation_id          uuid        not null,
    subject               text        not null,
    payload               jsonb       not null,
    enqueued_at           timestamptz not null,
    next_attempt_at       timestamptz not null,
    attempts              integer     not null default 0,
    claimed_until         timestamptz,
    claimed_by            text,
    published_at          timestamptz,
    dead_lettered_at      timestamptz,
    last_error            text,
    constraint outbox_subject_is_known check (subject in ('evt.content.document.extracted.v1', 'evt.platform.operation.reported.v1')),
    constraint outbox_payload_is_an_object check (jsonb_typeof(payload) = 'object'),
    constraint outbox_attempts_is_non_negative check (attempts >= 0),
    constraint outbox_claim_is_whole check ((claimed_until is null) = (claimed_by is null)),
    constraint outbox_claimed_by_is_bounded check (claimed_by is null or length(claimed_by) between 1 and 64),
    constraint outbox_last_error_is_safe check (last_error is null or (length(last_error) <= 512 and last_error !~ '[\r\n]')),
    constraint outbox_terminal_state_is_exclusive check (not (published_at is not null and dead_lettered_at is not null))
);

create index outbox_events_due_idx on extractor.outbox_events (next_attempt_at, enqueued_at) where published_at is null and dead_lettered_at is null;

create table extractor.render_budgets (
    utc_day    date    primary key,
    escalated  integer not null,
    constraint render_budgets_count_is_non_negative check (escalated >= 0)
);

comment on table extractor.render_budgets is 'Per-UTC-day count of published render commands; the escalation budget ceiling reads it atomically.';

comment on schema extractor is 'State owned exclusively by ratatoskr-extractor.';
comment on table extractor.artifacts is 'BlobRef fields only; raw artifact bytes never enter PostgreSQL.';
comment on table extractor.media_archives is 'Retention-bounded media archive accounting; BlobRef facts live in artifacts.';
comment on table extractor.candidates is 'Persistence shape for plan item 5; item 6 does not generate scores.';
