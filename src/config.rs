use config::{Config as ConfigLoader, ConfigError, File};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub gitlab: GitLabConfig,
    pub poller: PollerConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GitLabConfig {
    pub url: String,
    pub token: String,
    pub monitor_groups: Vec<String>,
    pub branch_filter_regex: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub skip_invalid_certs: Option<bool>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PollerConfig {
    pub interval_seconds: u64,
    pub backfill_days: i64,
    pub capacity: Option<i64>,
    pub ttl_seconds: Option<i64>,
}

impl Config {
    pub fn new() -> Result<Self, ConfigError> {
        let s = ConfigLoader::builder()
            .add_source(File::with_name("config"))
            .build()?;

        s.try_deserialize()
    }
}
