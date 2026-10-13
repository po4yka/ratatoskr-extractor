# Deploy Ratatoskr Extractor

The extractor is one `systemd` process. Raw artifacts stay in its private content-addressed tree;
there is no blob HTTP service.

```bash
sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin ratatoskr-extractor
# /etc/ratatoskr and the log directory belong to the shared `ratatoskr` group, exactly as
# ratatoskr-platform's deploy/README.md step 1 creates them; the unit names that group in
# SupplementaryGroups=. A per-service owner here would lock every other service out of both.
sudo getent group ratatoskr >/dev/null || sudo groupadd --system ratatoskr
sudo install -d -m 0750 -o root -g ratatoskr /etc/ratatoskr
sudo install -d -m 0700 -o ratatoskr-extractor -g ratatoskr-extractor \
  /mnt/nvme/ratatoskr/blobs/ratatoskr-extractor
sudo install -d -m 0770 -o root -g ratatoskr /mnt/nvme/ratatoskr/logs
# PostgreSQL is the `shared-postgres` container; there is no postgres user on the host.
docker exec -i shared-postgres psql -U postgres -d postgres -v ON_ERROR_STOP=1 < deploy/postgres/role.sql
# Set the database password outside Git, then install the private seed whose public key replaces
# the EXTRACTOR placeholder in the host NATS authorization file (see "Bus identity" below):
sudo install -m 0640 -o root -g ratatoskr-extractor /path/to/extractor.nkey \
  /etc/ratatoskr/extractor.nkey
# Telegram blob captures: the group that may read the Telegram service's blob root.
sudo getent group ratatoskr-telegram-blobs >/dev/null || sudo groupadd --system ratatoskr-telegram-blobs
sudo install -d -o ratatoskr-telegram-webhook -g ratatoskr-telegram-blobs -m 2750 \
  /mnt/nvme/ratatoskr/blobs/ratatoskr-telegram
# The binary of a commit on main is published, after the gate and the arm64 smoke test, as
# ghcr.io/po4yka/ratatoskr-extractor:sha-<commit> with SHA256SUMS and a provenance attestation;
# ratatoskr-workspace's deploy/ansible playbook installs exactly that. By hand, verify it on a machine
# with `oras` and `gh`, copy `out/ratatoskr-extractor` over, and never build on the host:
#   oras pull "ghcr.io/po4yka/ratatoskr-extractor:sha-$COMMIT" -o out
#   gh attestation verify "oci://ghcr.io/po4yka/ratatoskr-extractor:sha-$COMMIT" \
#     --repo po4yka/ratatoskr-extractor --source-digest "$COMMIT" \
#     --signer-workflow po4yka/ratatoskr-workspace/.github/workflows/reusable-release-artifact.yml
#   (cd out && sha256sum -c SHA256SUMS)
# The fallback is the same export stage built locally:
#   docker buildx build --platform linux/arm64 --target export --output type=local,dest=out .
sudo install -o root -g root -m 0755 out/ratatoskr-extractor /usr/local/bin/ratatoskr-extractor
sudo install -m 0640 -o root -g ratatoskr-extractor \
  deploy/systemd/extractor.conf.example /etc/ratatoskr/extractor.conf
sudo install -m 0644 deploy/systemd/ratatoskr-extractor.service /etc/systemd/system/
sudo systemctl daemon-reload
# The unit is WantedBy= and PartOf=ratatoskr.target, which ratatoskr-platform installs and enables
# (its deploy/README.md). Without that target the unit starts now but not at boot.
sudo systemctl enable --now ratatoskr-extractor
```

Verify the unit and the isolated admin plane:

```bash
systemd-analyze verify deploy/systemd/ratatoskr-extractor.service
sudo systemd-run --quiet --wait --pipe --collect --uid=ratatoskr-extractor \
  --property=EnvironmentFile=/etc/ratatoskr/extractor.conf \
  /usr/local/bin/ratatoskr-extractor check-config
curl --fail http://127.0.0.1:9088/health/ready
curl --fail http://127.0.0.1:9088/metrics
sudo systemctl show ratatoskr-extractor -p MemoryHigh -p MemoryMax -p CPUQuotaPerSecUSec -p TasksMax
```

Readiness requires the extractor-owned PostgreSQL schema, a connected JetStream client, and the two
fixed durables Edge provisions (see "Bus identity"). The process consumes
`cmd.content.capture.requested.v1`, publishes `evt.content.document.extracted.v1` and
`evt.platform.operation.reported.v1`, and publishes `cmd.content.render.requested.v1` when browser
escalation is enabled. Raw and Document IR bytes remain under the private blob root; PostgreSQL and
NATS carry `BlobRef` values only.

## Bus identity

The extractor authenticates with its own nkey and never creates topology. The authoritative
permissions are the `EXTRACTOR` and `EXTRACTOR_BROWSER_WORKER` stanzas of
`ratatoskr-platform/deploy/nats/ratatoskr.conf`; `deploy/nats/identity.conf` and
`deploy/nats/identity-browser-worker.conf` are byte-for-byte copies of those stanzas (comments,
whitespace and the nkey token aside) used by the authorized-broker tests, and a workspace check fails
when a copy diverges. Replace only the public-nkey placeholder in the host file; seeds stay in
`/etc/ratatoskr/extractor.nkey` and `/etc/ratatoskr/extractor-browser-worker.nkey`.

Each stanza lets its identity subscribe to one inbox prefix and nothing else: `_INBOX_extractor.>`
for the extractor and `_INBOX_extractor_browser_worker.>` for the worker. Every connection the
processes open sets that prefix, because a JetStream reply is delivered to the subscription that
matches the reply subject, and a shared `_INBOX.>` grant would let a compromised service read
another service's deliveries. A build from before this change cannot complete JetStream requests
under the new stanzas (the reply is never delivered and the call times out), so reload NATS and
deploy the services of one snapshot together.

Start order on a fresh broker: reload NATS with the new authorization, start `ratatoskr-edge`, then
start the extractor and the browser worker. Edge creates the `ratatoskr_commands` and
`ratatoskr_events` streams, the `ratatoskr_extractor_capture` and `ratatoskr_browser_worker` command
durables, the `ratatoskr_extractor_render_awaits` event durable and the
`browser_worker_completions` bucket. Until Edge has done so, the extractor refuses to become ready
and exits with a message naming the missing durable ("start ratatoskr-edge first"), and systemd
restarts it. `RATATOSKR__BUS__PROVISION_TOPOLOGY=true` (and `BROWSER_PROVISION_TOPOLOGY=true` for the
worker) lets a process create the topology itself; it exists for an unauthenticated development
broker and is refused together with an nkey seed. A publish refused by the broker never reaches the
client as an error: it appears as a missing acknowledgement, so check the NATS server log for a
`Publish Violation`.

## Browser worker

The worker is a separate process with its own identity and configuration. `deploy/systemd/browser-
worker.conf.example` is its `EnvironmentFile`: `BROWSER_NATS_URL`, `BROWSER_NKEY_SEED_PATH`,
`BROWSER_BLOBS_ROOT`, `BROWSER_DURABLE_NAME`, `BROWSER_COMPLETIONS_BUCKET` and
`BROWSER_MAX_JOBS_PER_PROCESS`, and a test loads it through the worker's own settings, so a key the
worker does not know fails the build. It sets no `BROWSER_PROVISION_TOPOLOGY`: Edge provisions the
`ratatoskr_browser_worker` durable and the `browser_worker_completions` bucket first, and the worker
refuses to run until they exist. The `browser` profile of `compose.yaml` is for development against
an unauthenticated broker only. A systemd unit for the worker is not provided yet.

## Outbox delivery and readiness

A fact row is published, retried forever with backoff, or undeliverable (see "Outbox delivery" in
`README.md`). Only a message larger than the broker accepts is undeliverable: the NATS limit is 8 MiB
(`max_payload` in `ratatoskr-platform/deploy/nats/ratatoskr.conf`), and the extractor stops enqueueing
a document event above `RATATOSKR__BUS__MAX_EVENT_BYTES` (default 8000000). A broker outage of any
length loses nothing.

`/health/ready` turns 503 when the publisher is refused repeatedly: the oldest unpublished row has
failed three times or is older than 300 seconds. Read `ratatoskr_extractor_outbox_publisher_unhealthy_total`
and the `last_error` class of the oldest row in `extractor.outbox_events`. A refused publish shows only
as a missing acknowledgement, so check the NATS server log for a `Publish Violation`; fixing the ACL is
enough, the relay needs no restart.

## Telegram blob captures

A capture whose payload names a blob reads the bytes from the Telegram service's blob root,
`RATATOSKR__BLOBS__TELEGRAM_ROOT` (default `/mnt/nvme/ratatoskr/blobs/ratatoskr-telegram`). The unit
reads it through `SupplementaryGroups=ratatoskr-telegram-blobs` and `ReadOnlyPaths=`; the extractor
never writes, renames or deletes there. The directory is created setgid
(`install -d -m 2750 -g ratatoskr-telegram-blobs`) so files the Telegram service writes inherit the
group. Without the group the run fails with the stable class `blob_unreadable`.

The unit intentionally does not use `IPAddressDeny=any`: public HTTP egress is the extractor's job.
The resolver and every redirect hop enforce the SSRF policy. Host firewall policy remains a separate
deployment check.
