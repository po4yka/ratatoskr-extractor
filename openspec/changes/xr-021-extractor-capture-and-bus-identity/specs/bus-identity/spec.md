# bus-identity Delta

## ADDED Requirements

### Requirement: Topology provisioning is an explicit development switch

`bus.provision_topology` SHALL default to false, SHALL be refused together with an nkey seed, and SHALL be the only mode in which the extractor creates streams or consumers. While it is false the durable name SHALL be the fixed `ratatoskr_extractor_capture`.

#### Scenario: defaults and refusals

- **WHEN** configuration is validated with the default, with provisioning plus a seed, and with a custom durable name without provisioning
- **THEN** the default is false, the second names `bus.provision_topology`, and the third is refused; a custom name is accepted when provisioning

### Requirement: The extractor works under its narrow identity

Against a broker that authorizes only the EXTRACTOR stanza, Extractor SHALL consume its Edge-provisioned capture durable, publish the document fact and the operation report with acknowledgement, and await render results through `ratatoskr_extractor_render_awaits`, and SHALL be unable to publish `evt.social.source.captured.v1` or read another service's durable. When a durable or stream is missing, readiness SHALL stay false and name what is missing.

#### Scenario: the authorized broker matrix

- **WHEN** an admin provisions topology and the extractor identity runs with `provision_topology=false`
- **THEN** one capture command is applied, both facts are acknowledged, a render request completes, and the two forbidden operations are refused

#### Scenario: the identity fragment has no broad grants

- **WHEN** `deploy/nats/identity.conf` is read
- **THEN** it contains neither `evt.>` nor `$JS.API.>` and equals the EXTRACTOR stanza

### Requirement: The browser worker authenticates and only gets its topology

The browser worker SHALL connect with an nkey when `BROWSER_NKEY_SEED_PATH` is set, SHALL fetch its durable and `browser_worker_completions` by get, and SHALL create them only when `BROWSER_PROVISION_TOPOLOGY` is true.

#### Scenario: the worker identity renders one job

- **WHEN** an admin provisions topology and the worker identity consumes one render command
- **THEN** the completion event is observed, the KV key is marked done, the delivery is acknowledged, and stream creation and `evt.platform.operation.reported.v1` publication are refused
