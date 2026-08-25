#![forbid(unsafe_code)]

//! Shared foundation for Ratatoskr Extractor.

mod config;

pub use config::{
    AdminConfig, BlobConfig, BusConfig, ConfigError, ConfigViolation, DatabaseConfig,
    ExtractorConfig, FetchConfig, LogFormat, OtlpConfig, ParserConfig, PdfConfig, ProvidersConfig,
    RenderConfig, ShutdownConfig, TelemetryConfig, YoutubeConfig, YoutubeMediaConfig,
    YoutubeTranscriptConfig, config_figment, load, load_from,
};
