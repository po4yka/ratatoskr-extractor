use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use figment::Figment;
use figment::error::Kind;
use figment::providers::{Env, Serialized};
use tracing_subscriber::EnvFilter;

const ENV_PREFIX: &str = "RATATOSKR__";

/// All validated process configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractorConfig {
    /// Operator listener configuration.
    pub admin: AdminConfig,
    /// Raw artifact storage configuration.
    pub blobs: BlobConfig,
    /// Extractor-owned `PostgreSQL` configuration.
    pub database: DatabaseConfig,
    /// Durable NATS command and event transport.
    pub bus: BusConfig,
    /// Network retrieval limits.
    pub fetch: FetchConfig,
    /// Bounded parse-once limits.
    pub parser: ParserConfig,
    /// Bounded process shutdown.
    pub shutdown: ShutdownConfig,
    /// Logging and trace export.
    pub telemetry: TelemetryConfig,
}

/// Operator listener configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminConfig {
    /// Address for health and metrics routes.
    pub bind: SocketAddr,
}

/// Extractor-owned artifact storage configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobConfig {
    /// Absolute content-addressed storage root.
    pub root: PathBuf,
}

/// Extractor-owned `PostgreSQL` pool configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// `PostgreSQL` URL, redacted from serialization and debug output.
    #[serde(default, skip_serializing)]
    pub url: secrecy::SecretString,
    /// Finite pool ceiling.
    pub max_connections: u32,
    /// Pool acquisition deadline.
    pub acquire_timeout_ms: u64,
}

/// Durable command/event bus configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusConfig {
    /// Credential-free NATS endpoint.
    pub url: String,
    /// Optional file containing the deployment nkey seed.
    pub nkey_seed_path: Option<PathBuf>,
    /// Durable capture consumer name.
    pub durable_name: String,
    /// Outbox and worker scheduler cadence.
    pub poll_interval_ms: u64,
    /// Maximum rows published in one pass.
    pub outbox_batch_size: i64,
    /// Work lease, longer than the network deadline.
    pub worker_lease_seconds: i32,
}

/// Parse-once resource ceilings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParserConfig {
    /// Maximum verified source bytes admitted to a parser.
    pub max_input_bytes: usize,
    /// Maximum nodes in one parsed DOM.
    pub max_dom_nodes: usize,
}

/// Bounded network retrieval configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchConfig {
    /// Maximum URL size before parsing.
    pub max_url_length: usize,
    /// Allowed destination ports.
    pub allowed_ports: Vec<u16>,
    /// Connection phase limit.
    pub connect_timeout_ms: u64,
    /// First response byte limit.
    pub first_byte_timeout_ms: u64,
    /// Idle interval between body chunks.
    pub read_idle_timeout_ms: u64,
    /// Whole operation limit.
    pub total_timeout_ms: u64,
    /// Maximum encoded body size.
    pub max_wire_bytes: u64,
    /// Maximum decoded body size.
    pub max_decoded_bytes: u64,
    /// Maximum redirect count.
    pub max_redirects: u16,
    /// Maximum retry count.
    pub max_retries: u16,
    /// Maximum simultaneous operations.
    pub global_concurrency: usize,
    /// Maximum simultaneous operations for one host.
    pub per_host_concurrency: usize,
}

/// Bounded process shutdown configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownConfig {
    /// Time allowed for admitted work to finish.
    pub grace_seconds: u64,
}

/// Logging and trace export configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryConfig {
    /// Log rendering mode.
    pub log_format: LogFormat,
    /// Structured log filter.
    pub log_filter: String,
    /// Optional OTLP collector configuration.
    pub otlp: Option<OtlpConfig>,
}

/// OTLP trace exporter configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OtlpConfig {
    /// Collector endpoint.
    pub endpoint: url::Url,
    /// Export request timeout.
    pub timeout_seconds: u64,
}

/// Supported log rendering modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    /// One JSON object per line.
    Json,
    /// Human-readable local output.
    Pretty,
}

/// A configuration failure that prevents startup.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The configuration provider could not deserialize a field.
    #[error("configuration could not be read")]
    Source(#[source] Box<figment::Error>),
    /// Parsed values violate one or more startup invariants.
    #[error("configuration is invalid: {} problem(s)", .0.len())]
    Invalid(Vec<ConfigViolation>),
}

/// One value-free semantic configuration problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigViolation {
    key: &'static str,
    rule: &'static str,
}

impl ExtractorConfig {
    /// Builds the typed scaffold used before environment overrides.
    #[must_use]
    pub fn built_in(blob_root: &Path) -> Self {
        Self {
            admin: AdminConfig {
                bind: SocketAddr::from(([127, 0, 0, 1], 9467)),
            },
            blobs: BlobConfig {
                root: blob_root.to_path_buf(),
            },
            database: DatabaseConfig {
                url: secrecy::SecretString::from(String::new()),
                max_connections: 10,
                acquire_timeout_ms: 5_000,
            },
            bus: BusConfig {
                url: "nats://127.0.0.1:4222".to_owned(),
                nkey_seed_path: None,
                durable_name: "ratatoskr_extractor_capture".to_owned(),
                poll_interval_ms: 100,
                outbox_batch_size: 32,
                worker_lease_seconds: 60,
            },
            fetch: FetchConfig {
                max_url_length: 8_192,
                allowed_ports: vec![80, 443],
                connect_timeout_ms: 5_000,
                first_byte_timeout_ms: 10_000,
                read_idle_timeout_ms: 10_000,
                total_timeout_ms: 30_000,
                max_wire_bytes: 25 * 1_024 * 1_024,
                max_decoded_bytes: 50 * 1_024 * 1_024,
                max_redirects: 10,
                max_retries: 2,
                global_concurrency: 64,
                per_host_concurrency: 8,
            },
            parser: ParserConfig {
                max_input_bytes: 50 * 1_024 * 1_024,
                max_dom_nodes: 250_000,
            },
            shutdown: ShutdownConfig { grace_seconds: 25 },
            telemetry: TelemetryConfig {
                log_format: LogFormat::Json,
                log_filter: "info,tower_http=info,hyper=warn,h2=warn".to_owned(),
                otlp: None,
            },
        }
    }
}

impl ConfigError {
    /// Returns an operator-facing startup report.
    #[must_use]
    pub fn report(&self) -> String {
        match self {
            Self::Source(error) => report_source(error),
            Self::Invalid(violations) => report_invalid(violations),
        }
    }

    /// Returns the `EX_CONFIG` process status.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        78
    }
}

impl From<figment::Error> for ConfigError {
    fn from(value: figment::Error) -> Self {
        Self::Source(Box::new(value))
    }
}

/// Loads process configuration from defaults and `RATATOSKR__` environment variables.
///
/// # Errors
///
/// Returns [`ConfigError`] when a field cannot be read.
pub fn load() -> Result<ExtractorConfig, ConfigError> {
    load_from(&config_figment())
}

/// Returns the provider stack used by [`load`].
#[must_use]
pub fn config_figment() -> Figment {
    Figment::from(Serialized::defaults(ExtractorConfig::built_in(Path::new(
        "",
    ))))
    .merge(Env::prefixed(ENV_PREFIX).split("__"))
}

/// Extracts configuration from a supplied provider stack.
///
/// # Errors
///
/// Returns [`ConfigError`] when a field cannot be read.
pub fn load_from(figment: &Figment) -> Result<ExtractorConfig, ConfigError> {
    let config = figment.extract()?;
    let violations = validate(&config);
    if violations.is_empty() {
        Ok(config)
    } else {
        Err(ConfigError::Invalid(violations))
    }
}

fn validate(config: &ExtractorConfig) -> Vec<ConfigViolation> {
    let mut violations = Vec::new();
    require(
        config.admin.bind.port() != 0,
        "admin.bind",
        "must use a non-zero port",
        &mut violations,
    );
    require(
        config.blobs.root.is_absolute(),
        "blobs.root",
        "must be an absolute path",
        &mut violations,
    );
    validate_database(&config.database, &mut violations);
    validate_bus(&config.bus, config.fetch.total_timeout_ms, &mut violations);
    validate_fetch(&config.fetch, &mut violations);
    for (valid, key) in [
        (config.parser.max_input_bytes > 0, "parser.max_input_bytes"),
        (config.parser.max_dom_nodes > 0, "parser.max_dom_nodes"),
    ] {
        require(valid, key, "must be greater than zero", &mut violations);
    }
    require(
        config.shutdown.grace_seconds > 0,
        "shutdown.grace_seconds",
        "must be greater than zero",
        &mut violations,
    );
    require(
        EnvFilter::try_new(&config.telemetry.log_filter).is_ok(),
        "telemetry.log_filter",
        "must be a valid tracing filter",
        &mut violations,
    );
    if let Some(otlp) = &config.telemetry.otlp {
        require(
            matches!(otlp.endpoint.scheme(), "http" | "https"),
            "telemetry.otlp.endpoint",
            "must use http or https",
            &mut violations,
        );
        require(
            otlp.timeout_seconds > 0,
            "telemetry.otlp.timeout_seconds",
            "must be greater than zero",
            &mut violations,
        );
    }
    violations
}

fn validate_database(config: &DatabaseConfig, violations: &mut Vec<ConfigViolation>) {
    use secrecy::ExposeSecret as _;

    let url = config.url.expose_secret();
    require(
        url.starts_with("postgres://") || url.starts_with("postgresql://"),
        "database.url",
        "must be a PostgreSQL URL",
        violations,
    );
    require(
        (1..=100).contains(&config.max_connections),
        "database.max_connections",
        "must be between 1 and 100",
        violations,
    );
    require(
        config.acquire_timeout_ms > 0,
        "database.acquire_timeout_ms",
        "must be greater than zero",
        violations,
    );
}

fn validate_bus(config: &BusConfig, fetch_timeout_ms: u64, violations: &mut Vec<ConfigViolation>) {
    let parsed = url::Url::parse(&config.url);
    let valid_url = parsed.as_ref().is_ok_and(|url| {
        matches!(url.scheme(), "nats" | "tls")
            && url.username().is_empty()
            && url.password().is_none()
    });
    require(
        valid_url,
        "bus.url",
        "must be a credential-free nats or tls URL",
        violations,
    );
    require(
        !config.durable_name.is_empty() && config.durable_name.len() <= 64,
        "bus.durable_name",
        "must contain 1 to 64 bytes",
        violations,
    );
    for (valid, key) in [
        (config.poll_interval_ms > 0, "bus.poll_interval_ms"),
        (
            (1..=1_000).contains(&config.outbox_batch_size),
            "bus.outbox_batch_size",
        ),
        (
            (1..=3_600).contains(&config.worker_lease_seconds),
            "bus.worker_lease_seconds",
        ),
    ] {
        require(valid, key, "must be within its finite range", violations);
    }
    let lease_ms = u64::try_from(config.worker_lease_seconds)
        .map_or(0, |seconds| seconds.saturating_mul(1_000));
    require(
        lease_ms > fetch_timeout_ms,
        "bus.worker_lease_seconds",
        "must exceed fetch.total_timeout_ms",
        violations,
    );
}

fn validate_fetch(config: &FetchConfig, violations: &mut Vec<ConfigViolation>) {
    require(
        config.max_url_length > 0,
        "fetch.max_url_length",
        "must be greater than zero",
        violations,
    );
    require(
        !config.allowed_ports.is_empty() && config.allowed_ports.iter().all(|port| *port != 0),
        "fetch.allowed_ports",
        "must contain only non-zero ports",
        violations,
    );
    for (valid, key) in [
        (config.connect_timeout_ms > 0, "fetch.connect_timeout_ms"),
        (
            config.first_byte_timeout_ms > 0,
            "fetch.first_byte_timeout_ms",
        ),
        (
            config.read_idle_timeout_ms > 0,
            "fetch.read_idle_timeout_ms",
        ),
        (config.total_timeout_ms > 0, "fetch.total_timeout_ms"),
        (config.max_wire_bytes > 0, "fetch.max_wire_bytes"),
        (config.max_decoded_bytes > 0, "fetch.max_decoded_bytes"),
        (config.max_redirects > 0, "fetch.max_redirects"),
        (config.max_retries > 0, "fetch.max_retries"),
        (config.global_concurrency > 0, "fetch.global_concurrency"),
        (
            config.per_host_concurrency > 0,
            "fetch.per_host_concurrency",
        ),
    ] {
        require(valid, key, "must be greater than zero", violations);
    }
    require(
        config.per_host_concurrency <= config.global_concurrency,
        "fetch.per_host_concurrency",
        "must not exceed global concurrency",
        violations,
    );
    require(
        config.max_decoded_bytes >= config.max_wire_bytes,
        "fetch.max_decoded_bytes",
        "must not be smaller than the wire-byte limit",
        violations,
    );
}

fn require(
    valid: bool,
    key: &'static str,
    rule: &'static str,
    violations: &mut Vec<ConfigViolation>,
) {
    if !valid {
        violations.push(ConfigViolation { key, rule });
    }
}

fn report_source(error: &figment::Error) -> String {
    let mut report =
        "ratatoskr-extractor: refusing to start; configuration could not be read.\n".to_owned();
    for problem in error.clone() {
        let key = error_key(&problem);
        let _ = writeln!(
            report,
            "  {key}\n      {}\n      {}",
            environment_key(&key),
            error_reason(&problem)
        );
    }
    report.push_str("Supplied values are never echoed.\n");
    report
}

fn report_invalid(violations: &[ConfigViolation]) -> String {
    let mut report = format!(
        "ratatoskr-extractor: refusing to start; {} configuration problem(s).\n",
        violations.len()
    );
    for violation in violations {
        let _ = writeln!(
            report,
            "  {}\n      {}\n      {}",
            violation.key,
            environment_key(violation.key),
            violation.rule
        );
    }
    report.push_str("Supplied values are never echoed.\n");
    report
}

fn error_key(error: &figment::Error) -> String {
    let path = error.path.join(".");
    match &error.kind {
        Kind::MissingField(name) if path.is_empty() => name.to_string(),
        Kind::MissingField(name) => format!("{path}.{name}"),
        _ if !path.is_empty() => path,
        Kind::UnknownField(name, _) => name.clone(),
        _ => "(unknown configuration key)".to_owned(),
    }
}

fn environment_key(key: &str) -> String {
    format!("{ENV_PREFIX}{}", key.replace('.', "__").to_uppercase())
}

const fn error_reason(error: &figment::Error) -> &'static str {
    match &error.kind {
        Kind::UnknownField(_, _) => "is not a configuration key of this process",
        Kind::MissingField(_) => "is required and was not supplied",
        _ => "could not be read as the type of this field",
    }
}
