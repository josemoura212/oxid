use std::{
    env,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
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
    pub cache: CacheSettings,
    pub rate_limit: RateLimitSettings,
    pub session: SessionSettings,
    pub analytics: AnalyticsSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionSettings {
    /// How long a session survives without being renewed.
    ///
    /// Fixed, not sliding. Sliding would mean a write to Redis on every
    /// authenticated request, and a tab left open would never expire.
    pub ttl_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApplicationSettings {
    pub host: IpAddr,
    pub port: u16,
    pub base_url: String,
    /// Metrics live on their own port, never on the public router.
    ///
    /// Traefik forwards everything that is not the front end to this service, so
    /// a `/metrics` route would publish request volumes, latency distributions
    /// and cache behaviour to the internet. A second listener is reachable from
    /// inside the cluster and nowhere else.
    pub metrics_port: u16,
}

impl ApplicationSettings {
    pub const fn addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }

    pub const fn metrics_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.metrics_port)
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
    /// Ceiling for a single query, applied per connection.
    ///
    /// With a small pool this is not a nicety. A query that hangs holds one of
    /// the connections for as long as it hangs, and there are only
    /// `max_connections` of them: eight stuck queries and the service stops
    /// answering entirely. The timeout turns "everything is down" into "that
    /// one query failed".
    pub statement_timeout_ms: u64,
}

impl DatabaseSettings {
    pub fn connect_options(&self) -> PgConnectOptions {
        let ssl_mode = if self.require_ssl {
            PgSslMode::Require
        } else {
            PgSslMode::Prefer
        };

        // `options` sets Postgres runtime parameters on every connection the
        // pool opens. Doing it here rather than on the server keeps the ceiling
        // with the application that needs it: the same database can serve a
        // migration or a manual session that legitimately runs longer.
        PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .username(&self.username)
            .password(self.password.expose_secret())
            .database(&self.database_name)
            .ssl_mode(ssl_mode)
            .options([("statement_timeout", self.statement_timeout_ms.to_string())])
    }

    pub const fn acquire_timeout(&self) -> Duration {
        Duration::from_secs(self.acquire_timeout_seconds)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheSettings {
    pub host: String,
    pub port: u16,
    /// Positive entries never expire — a shortcode is immutable, so there is
    /// nothing to invalidate. Only the "does not exist" sentinel gets a TTL,
    /// because that answer can stop being true the moment someone shortens a URL.
    pub negative_ttl_seconds: u64,
    pub connect_timeout_seconds: u64,
}

impl CacheSettings {
    pub fn url(&self) -> String {
        format!("redis://{}:{}", self.host, self.port)
    }

    pub const fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_seconds)
    }
}

/// Where click events go.
///
/// `off` is not a placeholder — it is the state the load-test stages run in, so
/// analytics never contaminates a latency measurement, the same reason
/// `Cache::disabled()` exists. The ClickHouse settings are only read when the
/// backend selects it, hence `#[serde(default)]`: an `off` deployment does not
/// have to carry a connection block it never uses.
#[derive(Debug, Clone, Deserialize)]
pub struct AnalyticsSettings {
    pub backend: AnalyticsBackend,
    #[serde(default)]
    pub clickhouse: ClickHouseSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnalyticsBackend {
    Off,
    ClickHouse,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClickHouseSettings {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: SecretString,
    pub database: String,
}

/// Written by hand rather than derived: `SecretString` has no `Default`, and
/// this default is only ever the placeholder for an `off` backend that never
/// reads it.
impl Default for ClickHouseSettings {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 0,
            user: String::new(),
            password: SecretString::from(String::new()),
            database: String::new(),
        }
    }
}

impl ClickHouseSettings {
    /// The HTTP interface URL. Plain HTTP on purpose: ClickHouse sits inside the
    /// cluster reachable only by the API, the same posture as Postgres and Redis.
    pub fn url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    pub fn password(&self) -> &str {
        self.password.expose_secret()
    }
}

/// Limits on the two routes that cost something to abuse.
///
/// The redirect is deliberately unlimited: it is the path the cache absorbs, the
/// one stages 9 and 10 push to 11k req/s, and throttling it would punish exactly
/// the traffic the system exists to serve. Writing costs a row; logging in costs
/// an Argon2 verification.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct RateLimitSettings {
    /// Sustained rate, per client key.
    pub shorten_per_second: u64,
    /// How much a client may exceed the sustained rate before being throttled.
    pub shorten_burst: u32,

    /// Login is limited far harder than writing, and not because of rows.
    ///
    /// Each attempt spends ~19 MiB and tens of milliseconds of Argon2 by
    /// design — and the decoy path means an attacker gets that cost without
    /// needing a real account. On a small node a few dozen attempts a second
    /// are enough to saturate CPU, so this limit is a denial-of-service control
    /// first and a credential-stuffing control second.
    pub login_per_second: u64,
    pub login_burst: u32,

    /// How many password hashes may run at once, across every caller.
    ///
    /// The per-IP limit above depends on correctly identifying the client, and
    /// behind a CDN that has already failed once without a symptom. This does not
    /// depend on it: it bounds the total Argon2 in flight, so a flood becomes a
    /// queue rather than a saturated node.
    ///
    /// One is not as restrictive as it reads. A verification is tens of
    /// milliseconds, so a single slot still serves more logins per second than
    /// `login_per_second` allows — raising it would only widen the damage a
    /// successful flood can do.
    pub hash_concurrency: usize,

    /// How long a request waits for a slot before answering 503.
    ///
    /// Bounded, because waiting forever trades a saturated CPU for an unbounded
    /// queue — which fails later, less legibly, and while holding connections.
    pub hash_wait_ms: u64,
}

impl RateLimitSettings {
    pub const fn hash_wait(&self) -> Duration {
        Duration::from_millis(self.hash_wait_ms)
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
            other => {
                anyhow::bail!("invalid APP_ENVIRONMENT: {other:?}. Use `local` or `production`")
            }
        }
    }
}

/// `configuration/` lives at the workspace root, but the cwd varies:
/// `cargo run` starts at the root, `cargo test` at the crate directory.
/// Walks up the tree until it finds one.
fn config_dir() -> anyhow::Result<PathBuf> {
    let cwd = env::current_dir().context("could not determine the current directory")?;

    cwd.ancestors()
        .map(|dir| dir.join("configuration"))
        .find(|candidate| candidate.is_dir())
        .with_context(|| {
            format!(
                "`configuration/` directory not found starting from {}",
                cwd.display()
            )
        })
}

/// Precedence: `base.yaml` → `<environment>.yaml` → `APP_*` variables.
pub fn load() -> anyhow::Result<Settings> {
    let dir = config_dir()?;
    let environment = Environment::from_env()?;

    config::Config::builder()
        .add_source(config::File::from(dir.join("base")).required(true))
        .add_source(config::File::from(dir.join(environment.as_str())).required(true))
        // `prefix_separator` must be explicit: without it `config` reuses the
        // `separator` for the prefix and would demand `APP__APPLICATION__PORT`.
        .add_source(
            config::Environment::with_prefix("app")
                .prefix_separator("_")
                .separator("__"),
        )
        .build()
        .context("failed to assemble the configuration sources")?
        .try_deserialize()
        .context("invalid configuration")
}
