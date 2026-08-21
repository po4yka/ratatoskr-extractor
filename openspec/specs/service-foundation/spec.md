# Extractor service foundation

## Purpose

Defines the process, configuration, diagnostics, and deployment behavior that every first-stage extractor operation relies on.

## Requirements

### Requirement: Configuration is typed and validated before startup
The extractor SHALL read one typed configuration tree from built-in defaults and `RATATOSKR__` environment variables, SHALL reject unknown keys and invalid values, and SHALL report configuration errors without including supplied values.

#### Scenario: Invalid configuration is safe to diagnose
- **WHEN** an environment value has the wrong type or violates a configured limit
- **THEN** configuration loading fails before a listener binds and the operator report names the variable and rule without the supplied value

### Requirement: Security and resource defaults are finite
The default configuration SHALL set finite fetch deadlines, body limits, redirect and retry limits, concurrency limits, shutdown time, and a loopback admin listener. A runtime option SHALL NOT disable URL or SSRF validation.

#### Scenario: Empty environment keeps finite limits
- **WHEN** the typed defaults are loaded without environment overrides
- **THEN** every network, body, redirect, retry, concurrency, and shutdown limit is finite and greater than zero where zero would disable the control

### Requirement: The operator plane reports process health
The extractor SHALL expose liveness, readiness, metrics, and build identity on a separate admin listener. Liveness SHALL depend only on the running process. Readiness SHALL fail before startup completes and after shutdown starts. Every admin response SHALL prohibit caching.

#### Scenario: Readiness follows the process lifecycle
- **WHEN** the admin router is queried before startup, after startup, and after drain begins
- **THEN** liveness stays successful while readiness changes from unavailable to available and back to unavailable, with `Cache-Control: no-store`

### Requirement: Telemetry is structured and bounded
The extractor SHALL install one process-wide telemetry pipeline, expose Prometheus metrics, and emit only a closed set of structured fields. Telemetry SHALL NOT record raw URLs, URL queries, headers, response bodies, filesystem paths, credentials, or free-form remote errors.

#### Scenario: A fetch failure does not leak its target
- **WHEN** a fetch failure is recorded for a URL that contains a secret query value
- **THEN** captured telemetry contains the bounded failure class and contains neither the URL nor the secret value

### Requirement: Shutdown has one bounded deadline
On termination, the extractor SHALL become unready, stop accepting new work, allow admitted work to finish within one configured deadline, cancel remaining work, and terminate its owned tasks.

#### Scenario: Shutdown refuses new work before waiting
- **WHEN** shutdown begins while one operation remains admitted
- **THEN** readiness fails immediately, a new operation is refused, and the existing operation receives no more than the configured shutdown allowance

### Requirement: Expected failures are typed
Configuration, URL, policy, DNS, transport, timeout, resource-limit, cache, and artifact failures SHALL remain distinguishable without a panic or a string-matched public API.

#### Scenario: Callers can branch on a policy denial
- **WHEN** an otherwise valid URL resolves to a prohibited address
- **THEN** the caller receives a policy-denied error variant rather than a DNS, transport, or internal error
