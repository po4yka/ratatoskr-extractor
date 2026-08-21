#![forbid(unsafe_code)]

//! Shared foundation for Ratatoskr Extractor.

mod config;

pub use config::{
    AdminConfig, BlobConfig, ConfigError, ConfigViolation, ExtractorConfig, FetchConfig, LogFormat,
    OtlpConfig, ShutdownConfig, TelemetryConfig, config_figment, load, load_from,
};
