use crate::error::Result;
use config::{Config, File};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Settings {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub url: String,
}

impl Settings {
    pub fn new() -> Result<Self> {
        let config = Config::builder()
            .add_source(File::with_name("defaults"))
            .add_source(File::with_name("config").required(false))
            .build()?;

        Ok(config.try_deserialize()?)
    }
}
