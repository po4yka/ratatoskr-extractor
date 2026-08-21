#![forbid(unsafe_code)]

//! Shared foundation for Ratatoskr Extractor.

mod config;

pub use config::{
    AdminConfig, BlobConfig, BusConfig, ConfigError, ConfigViolation, DatabaseConfig,
    ExtractorConfig, FetchConfig, LogFormat, OtlpConfig, ParserConfig, ShutdownConfig,
    TelemetryConfig, config_figment, load, load_from,
};
