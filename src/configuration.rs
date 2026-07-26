use std::{
    env,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use anyhow::Context;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use sqlx::postgres::{PgConnectOptions, PgSslMode};

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub application: ApplicationSettings,
    pub database: DatabaseSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApplicationSettings {
    pub host: IpAddr,
    pub port: u16,
    pub base_url: String,
}

impl ApplicationSettings {
    pub const fn addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseSettings {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: SecretString,
    pub database_name: String,
    pub require_ssl: bool,
    pub max_connections: u32,
    pub acquire_timeout_seconds: u64,
}

impl DatabaseSettings {
    pub fn connect_options(&self) -> PgConnectOptions {
        let ssl_mode = if self.require_ssl {
            PgSslMode::Require
        } else {
            PgSslMode::Prefer
        };

        PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .username(&self.username)
            .password(self.password.expose_secret())
            .database(&self.database_name)
            .ssl_mode(ssl_mode)
    }

    pub const fn acquire_timeout(&self) -> Duration {
        Duration::from_secs(self.acquire_timeout_seconds)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Local,
    Production,
}

impl Environment {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Production => "production",
        }
    }

    fn from_env() -> anyhow::Result<Self> {
        let raw = env::var("APP_ENVIRONMENT").unwrap_or_else(|_| Self::Local.as_str().to_owned());

        match raw.to_lowercase().as_str() {
            "local" => Ok(Self::Local),
            "production" => Ok(Self::Production),
            outro => {
                anyhow::bail!("APP_ENVIRONMENT inválido: {outro:?}. Use `local` ou `production`")
            }
        }
    }
}

/// Precedência: `base.yaml` → `<ambiente>.yaml` → variáveis `APP_*`.
pub fn load() -> anyhow::Result<Settings> {
    let base_path = env::current_dir().context("não foi possível determinar o diretório atual")?;
    let dir = base_path.join("configuration");
    let environment = Environment::from_env()?;

    config::Config::builder()
        .add_source(config::File::from(dir.join("base")).required(true))
        .add_source(config::File::from(dir.join(environment.as_str())).required(true))
        // `prefix_separator` precisa ser explícito: sem ele o `config` reaproveita
        // o `separator` para o prefixo e passaria a exigir `APP__APPLICATION__PORT`.
        .add_source(
            config::Environment::with_prefix("app")
                .prefix_separator("_")
                .separator("__"),
        )
        .build()
        .context("falha ao montar as fontes de configuração")?
        .try_deserialize()
        .context("configuração inválida")
}
