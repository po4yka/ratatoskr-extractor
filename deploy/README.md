# Deploy Ratatoskr Extractor

The extractor is one `systemd` process. Raw artifacts stay in its private content-addressed tree;
there is no blob HTTP service.

```bash
sudo useradd --system --user-group --no-create-home --shell /usr/sbin/nologin ratatoskr-extractor
sudo install -d -m 0750 -o root -g ratatoskr-extractor /etc/ratatoskr
sudo install -d -m 0700 -o ratatoskr-extractor -g ratatoskr-extractor \
  /mnt/nvme/ratatoskr/blobs/ratatoskr-extractor
sudo install -d -m 0770 -o root -g ratatoskr-extractor /mnt/nvme/ratatoskr/logs
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
curl --fail http://127.0.0.1:9467/health/ready
curl --fail http://127.0.0.1:9467/metrics
sudo systemctl show ratatoskr-extractor -p MemoryHigh -p MemoryMax -p CPUQuotaPerSecUSec -p TasksMax
```

The unit intentionally does not use `IPAddressDeny=any`: public HTTP egress is the extractor's job.
The resolver and every redirect hop enforce the SSRF policy. Host firewall policy remains a separate
deployment check.
