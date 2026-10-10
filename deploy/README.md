# Deploy Ratatoskr Extractor

The extractor is one `systemd` process. Raw artifacts stay in its private content-addressed tree;
there is no blob HTTP service.

```bash
sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin ratatoskr-extractor
sudo install -d -m 0750 -o root -g ratatoskr-extractor /etc/ratatoskr
sudo install -d -m 0700 -o ratatoskr-extractor -g ratatoskr-extractor \
  /mnt/nvme/ratatoskr/blobs/ratatoskr-extractor
sudo install -d -m 0770 -o root -g ratatoskr-extractor /mnt/nvme/ratatoskr/logs
sudo -u postgres psql -f deploy/postgres/role.sql
# Set the database password outside Git, then install the private seed whose public key replaces
# the EXTRACTOR placeholder in the host NATS authorization file (see "Bus identity" below):
sudo install -m 0640 -o root -g ratatoskr-extractor /path/to/extractor.nkey \
  /etc/ratatoskr/extractor.nkey
# Telegram blob captures: the group that may read the Telegram service's blob root.
sudo getent group ratatoskr-telegram-blobs >/dev/null || sudo groupadd --system ratatoskr-telegram-blobs
sudo install -d -o ratatoskr-telegram-webhook -g ratatoskr-telegram-blobs -m 2750 \
  /mnt/nvme/ratatoskr/blobs/ratatoskr-telegram
sudo install -m 0755 target/release/ratatoskr-extractor /usr/local/bin/ratatoskr-extractor
sudo install -m 0640 -o root -g ratatoskr-extractor \
  deploy/systemd/extractor.conf.example /etc/ratatoskr/extractor.conf
sudo install -m 0644 deploy/systemd/ratatoskr-extractor.service /etc/systemd/system/
sudo systemctl daemon-reload
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
