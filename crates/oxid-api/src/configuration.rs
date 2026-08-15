use std::{
    env,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
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
    pub email: EmailSettings,
}

/// Which mailer is wired, and what the wired one needs.
///
/// Same shape as [`AnalyticsSettings`], and for the same reason: the provider is
/// an outbound dependency that must be absent in development and in tests. Every
/// signup test would otherwise spend real Resend quota to deliver a link nobody
/// reads, and a laptop with no key would fail to boot.
#[derive(Debug, Clone, Deserialize)]
pub struct EmailSettings {
    pub backend: EmailBackend,

    /// Whether an unconfirmed address may sign in.
    ///
    /// **Turning this off reopens account enumeration, and there is no way to
    /// have both.** With confirmation required, a signup writes nothing a login
    /// can probe: a fresh account cannot sign in, so the answer is the same for a
    /// free address and a taken one. Without it, the account created by a signup
    /// works immediately — so `signup` then `login` with a password of your
    /// choosing succeeds on a free address and fails on a registered one, which
    /// is two requests per address and no timing involved.
    ///
    /// It exists because development and tests need a way through without a mail
    /// provider. Production keeps it on, and [`Settings::warn_about_tradeoffs`]
    /// says so out loud at boot when it does not.
    pub require_confirmation: bool,
    /// Where the links in the messages point. Separate from
    /// [`ApplicationSettings::base_url`] because the API and the front end can
    /// live on different hosts, and it is the front end a person clicks into.
    pub site_url: String,
    #[serde(default)]
    pub resend: ResendSettings,
    #[serde(default)]
    pub cloudflare: CloudflareSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmailBackend {
    Off,
    Resend,
    /// Cloudflare Email Service, over its REST API.
    ///
    /// No Worker involved: the binding is one of three ways in, and the HTTP one
    /// is callable from anywhere — the same shape the Resend client already uses.
    ///
    /// Its quota is **adaptive** rather than published: an account "starts with a
    /// conservative daily quota and scales up based on sending behaviour". That
    /// is the reason both backends exist rather than one replacing the other —
    /// hitting an unpublished ceiling on a confirmation link is somebody unable
    /// to sign in, and switching back has to be an environment variable rather
    /// than a deploy.
    Cloudflare,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CloudflareSettings {
    pub api_token: SecretString,
    pub account_id: String,
    /// The `From` address. Has to be on a domain verified in Cloudflare, the same
    /// requirement Resend makes.
    pub from: String,
}

/// By hand, for the same reason as [`ResendSettings::default`].
impl Default for CloudflareSettings {
    fn default() -> Self {
        Self {
            api_token: SecretString::from(String::new()),
            account_id: String::new(),
            from: String::new(),
        }
    }
}

impl CloudflareSettings {
    pub fn api_token(&self) -> &str {
        self.api_token.expose_secret()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResendSettings {
    pub api_key: SecretString,
    /// The `From` address. Its domain has to carry SPF, DKIM and DMARC in Resend
    /// before any of this is worth sending — without them the confirmation link
    /// lands in spam and the account looks broken rather than unconfirmed.
    pub from: String,
}

/// By hand rather than derived: `SecretString` has no `Default`, and this is only
/// ever the placeholder for an `off` backend that never reads it.
impl Default for ResendSettings {
    fn default() -> Self {
        Self {
            api_key: SecretString::from(String::new()),
            from: String::new(),
        }
    }
}

impl ResendSettings {
    pub fn api_key(&self) -> &str {
        self.api_key.expose_secret()
    }
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

    /// The tightest limit in the file, and the only one where the cost lands on
    /// somebody else.
    ///
    /// Every call to "resend my confirmation" or "I forgot my password" puts a
    /// message in a stranger's inbox. Unlimited, the pair is a mail-bombing tool
    /// pointed at any address an attacker chooses — and each send also spends
    /// provider quota this project pays for.
    ///
    /// Deliberately per-second-and-burst like the others rather than a daily cap
    /// per address. A daily cap keyed by e-mail would need state per address and
    /// would itself be an oracle: "you have asked too many times" is only true
    /// for an address that exists.
    pub email_per_second: u64,
    pub email_burst: u32,
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

    pub fn from_env() -> anyhow::Result<Self> {
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

/// Everything wrong with a configuration, reported together.
///
/// A `Vec` rather than the first problem found, because fixing environment
/// variables one restart at a time is miserable: five mistakes should cost one
/// boot, not five. This is the same reason a compiler does not stop at the first
/// error.
#[derive(Debug)]
pub struct Invalid(Vec<String>);

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "the configuration cannot be used:")?;

        for problem in &self.0 {
            writeln!(f, "  - {problem}")?;
        }

        Ok(())
    }
}

impl std::error::Error for Invalid {}

impl Invalid {
    pub fn problems(&self) -> &[String] {
        &self.0
    }
}

/// A URL that has to be usable as one, not merely present.
///
/// Checked because `site_url` ends up inside every confirmation link. Empty or
/// scheme-less, the link is unclickable and the failure surfaces in somebody
/// else's inbox — the slowest possible place to learn about a typo.
fn check_url(problems: &mut Vec<String>, name: &str, value: &str) {
    if value.trim().is_empty() {
        problems.push(format!("{name} is empty"));
    } else if !value.starts_with("http://") && !value.starts_with("https://") {
        problems.push(format!(
            "{name} must start with http:// or https://, got {value:?}"
        ));
    }
}

fn check_present(problems: &mut Vec<String>, name: &str, value: &str) {
    if value.trim().is_empty() {
        problems.push(format!(
            "{name} is required by the selected backend and is empty"
        ));
    }
}

/// Something a configuration gives up, named rather than described inline.
///
/// An enum instead of a `warn!` at the point of decision, because the decision is
/// worth testing and a log line is not: a function that only writes to a logger
/// can be asserted on by nobody, and this is exactly the kind of guard that stops
/// firing without anyone noticing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tradeoff {
    /// An account works the moment it is created, so signup followed by login
    /// tells a free address from a registered one.
    EnumerationOpen,
    /// Confirmation and reset links go to the log instead of to anybody.
    NobodyReceivesMail,
}

impl Tradeoff {
    pub const fn message(self) -> &'static str {
        match self {
            Self::EnumerationOpen => {
                "email confirmation is OFF: an account works the moment it is created, so                  signup followed by login tells a free address from a registered one. Account                  enumeration is open while this stays off."
            }
            Self::NobodyReceivesMail => {
                "the mailer is OFF: confirmation and password-reset links are written to this                  log instead of being sent, and nobody receives them."
            }
        }
    }
}

impl Settings {
    /// Refuses a configuration that deserializes but cannot work.
    ///
    /// Serde only proves the fields are *there*. A backend selected with an empty
    /// credential passes that bar and then fails at send time — which is minutes
    /// or hours later, in a background task, to a person who is waiting on an
    /// e-mail. Everything checked here is something that would otherwise be
    /// discovered far away from its cause.
    pub fn validate(&self, environment: Environment) -> Result<(), Invalid> {
        let mut problems = Vec::new();

        // **Confirmation required with no provider is a dead end — in production.**
        // Accounts get created that nobody can confirm, and nobody can sign in.
        //
        // Not an error on a laptop, and that exception is not laziness: the
        // disabled mailer logs the whole message, link included, precisely so the
        // flow stays completable by hand without a provider account. On a
        // developer's terminal that is a working path; in a cluster nobody reads
        // pod logs to activate their own account.
        if environment == Environment::Production
            && self.email.require_confirmation
            && self.email.backend == EmailBackend::Off
        {
            problems.push(
                "email.require_confirmation is on but email.backend is off: accounts would be \
                 created that nobody can confirm, and nobody could sign in. Select a provider, \
                 or turn confirmation off."
                    .to_owned(),
            );
        }

        check_url(
            &mut problems,
            "application.base_url",
            &self.application.base_url,
        );
        check_url(&mut problems, "email.site_url", &self.email.site_url);

        match self.email.backend {
            EmailBackend::Off => {}
            EmailBackend::Resend => {
                check_present(
                    &mut problems,
                    "email.resend.api_key",
                    self.email.resend.api_key(),
                );
                check_present(&mut problems, "email.resend.from", &self.email.resend.from);
            }
            EmailBackend::Cloudflare => {
                check_present(
                    &mut problems,
                    "email.cloudflare.api_token",
                    self.email.cloudflare.api_token(),
                );
                check_present(
                    &mut problems,
                    "email.cloudflare.account_id",
                    &self.email.cloudflare.account_id,
                );
                check_present(
                    &mut problems,
                    "email.cloudflare.from",
                    &self.email.cloudflare.from,
                );
            }
        }

        if self.analytics.backend == AnalyticsBackend::ClickHouse {
            check_present(
                &mut problems,
                "analytics.clickhouse.host",
                &self.analytics.clickhouse.host,
            );
        }

        // A pool of zero accepts no connection and every request waits for a slot
        // that never frees. It deserializes fine.
        if self.database.max_connections == 0 {
            problems.push("database.max_connections is 0, so no query can ever run".to_owned());
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(Invalid(problems))
        }
    }

    /// What this configuration gives up, in the environment it runs in.
    ///
    /// Empty outside production on purpose: both of these are the normal way to
    /// work on a laptop, and warning about them there would train everyone to
    /// ignore the warning that matters.
    pub fn tradeoffs(&self, environment: Environment) -> Vec<Tradeoff> {
        if environment != Environment::Production {
            return Vec::new();
        }

        let mut found = Vec::new();

        if !self.email.require_confirmation {
            found.push(Tradeoff::EnumerationOpen);
        }

        if self.email.backend == EmailBackend::Off {
            found.push(Tradeoff::NobodyReceivesMail);
        }

        found
    }

    /// Says them out loud at boot.
    ///
    /// Not a refusal: both are legitimate somewhere. What they cannot be is
    /// silent — a production deploy that stopped requiring confirmation should
    /// say so in the first ten lines of its log rather than be discovered from a
    /// support message.
    pub fn warn_about_tradeoffs(&self, environment: Environment) {
        for tradeoff in self.tradeoffs(environment) {
            tracing::warn!("{}", tradeoff.message());
        }
    }
}

/// The layering, without reading the environment for which layer to pick.
///
/// Separate from [`load`] so the tests can assemble a named environment without
/// touching a process-wide variable. That matters more than it sounds: what
/// breaks in this file is never the Rust, it is a `.yaml` naming a block whose
/// required field nothing supplies, and that is only caught by deserializing the
/// real files.
fn layers(dir: &Path, environment: &str) -> config::ConfigBuilder<config::builder::DefaultState> {
    config::Config::builder()
        .add_source(config::File::from(dir.join("base")).required(true))
        .add_source(config::File::from(dir.join(environment)).required(true))
        // `prefix_separator` must be explicit: without it `config` reuses the
        // `separator` for the prefix and would demand `APP__APPLICATION__PORT`.
        .add_source(
            config::Environment::with_prefix("app")
                .prefix_separator("_")
                .separator("__"),
        )
}

/// Precedence: `base.yaml` → `<environment>.yaml` → `APP_*` variables.
pub fn load() -> anyhow::Result<Settings> {
    let dir = config_dir()?;
    let environment = Environment::from_env()?;

    layers(&dir, environment.as_str())
        .build()
        .context("failed to assemble the configuration sources")?
        .try_deserialize()
        .context("invalid configuration")
}

#[cfg(test)]
mod tests {
    use super::{Settings, config_dir, layers};

    /// Everything the cluster injects that no `.yaml` carries — the `oxid-config`
    /// `ConfigMap` plus the three Secrets — kept in one place so adding a secret
    /// means updating this list, and the production test keeps meaning "this is
    /// what the Deployment actually provides".
    const CLUSTER_ENV: [(&str, &str); 7] = [
        ("database.password", "x"),
        ("database.username", "oxid"),
        ("database.database_name", "oxid"),
        ("analytics.backend", "clickhouse"),
        ("analytics.clickhouse.password", "x"),
        // Deliberately not shaped like a Resend key: the `re_` prefix is what
        // their scanner matches, and a placeholder that trips it costs a
        // review cycle to explain every time.
        ("email.resend.api_key", "placeholder"),
        // Both halves. A provider block is all-or-nothing: naming one field
        // creates the block and makes the rest required, which is exactly why no
        // `.yaml` names either provider any more.
        ("email.resend.from", "oxid <no-reply@oxid.uk>"),
    ];

    fn assemble(environment: &str, overrides: &[(&str, &str)]) -> anyhow::Result<Settings> {
        let dir = config_dir()?;
        let mut builder = layers(&dir, environment);

        for (key, value) in overrides {
            builder = builder.set_override(*key, *value)?;
        }

        Ok(builder.build()?.try_deserialize()?)
    }

    /// A laptop with no secrets in the environment has to boot. This is the case
    /// the empty `resend:` block broke: naming the block makes `api_key` required
    /// everywhere, including where nothing is ever sent.
    #[test]
    fn local_loads_with_nothing_in_the_environment() {
        let settings = assemble("local", &[]).expect("local must load unaided");

        assert_eq!(settings.email.backend, super::EmailBackend::Off);
    }

    /// The production files plus exactly what the Deployment injects. This is the
    /// test that would have caught the ClickHouse password, twice.
    #[test]
    fn production_loads_with_the_secrets_the_deployment_injects() {
        let settings = assemble("production", &CLUSTER_ENV).expect("production must load");

        assert_eq!(settings.email.backend, super::EmailBackend::Resend);
        assert_eq!(settings.email.site_url, "https://oxid.uk");
        assert!(
            !settings.email.resend.from.is_empty(),
            "a From address is required before Resend accepts anything"
        );
    }

    /// Production selects Resend, so the key is required — and its absence has to
    /// stop the boot rather than come up with a mailer that cannot mail.
    ///
    /// This is also why the migration Job overrides the backend to `off`: it
    /// inherits the same `ConfigMap` and would otherwise need a credential it never
    /// uses.
    #[test]
    fn production_without_the_api_key_refuses_to_boot() {
        let without: Vec<(&str, &str)> = CLUSTER_ENV
            .into_iter()
            .filter(|(key, _)| *key != "email.resend.api_key")
            .collect();

        assert!(
            assemble("production", &without).is_err(),
            "a missing Resend key must fail the boot, not be defaulted away"
        );
    }

    /// The other provider selects and deserializes from the environment alone.
    ///
    /// Cloudflare's block has no entry in any `.yaml` — it exists only if the
    /// deployment injects it, which is what makes switching providers an
    /// environment change rather than a deploy.
    #[test]
    fn cloudflare_can_be_selected_entirely_from_the_environment() {
        let settings = assemble(
            "production",
            &[
                ("database.password", "x"),
                ("database.username", "oxid"),
                ("database.database_name", "oxid"),
                ("analytics.backend", "off"),
                ("analytics.clickhouse.password", ""),
                ("email.backend", "cloudflare"),
                ("email.cloudflare.api_token", "placeholder"),
                ("email.cloudflare.account_id", "abc123"),
                ("email.cloudflare.from", "oxid <no-reply@oxid.uk>"),
            ],
        )
        .expect("cloudflare must select from the environment alone");

        assert_eq!(settings.email.backend, super::EmailBackend::Cloudflare);
        assert_eq!(settings.email.cloudflare.account_id, "abc123");
    }

    /// Confirmation is required unless something turns it off, and nothing in the
    /// files does. A default that silently let unconfirmed accounts sign in would
    /// be the enumeration hole arriving by omission.
    #[test]
    fn confirmation_is_required_by_default_everywhere() {
        for environment in ["local", "production"] {
            let overrides: Vec<(&str, &str)> = CLUSTER_ENV.into_iter().collect();
            let settings = assemble(environment, &overrides).expect("must load");

            assert!(
                settings.email.require_confirmation,
                "{environment} does not require confirmation"
            );
        }
    }

    // --- the validator ---
    //
    // Every one of these is a configuration that *deserializes*. Serde proves the
    // fields are there; these prove they mean something.

    fn production_with(extra: &[(&str, &str)]) -> Settings {
        let mut overrides: Vec<(&str, &str)> = CLUSTER_ENV.into_iter().collect();
        overrides.extend_from_slice(extra);

        assemble("production", &overrides).expect("must deserialize")
    }

    fn problems(settings: &Settings, environment: super::Environment) -> Vec<String> {
        settings
            .validate(environment)
            .err()
            .map(|invalid| invalid.problems().to_vec())
            .unwrap_or_default()
    }

    /// What actually ships has to pass. If this fails, the validator is wrong
    /// rather than the configuration.
    #[test]
    fn the_shipped_configurations_are_valid() {
        assemble("production", &CLUSTER_ENV)
            .expect("must deserialize")
            .validate(super::Environment::Production)
            .expect("production must be valid");

        assemble("local", &[])
            .expect("must deserialize")
            .validate(super::Environment::Local)
            .expect("local must be valid");
    }

    /// A backend selected with an empty credential deserializes fine and then
    /// fails at send time — in a background task, hours later, to somebody
    /// waiting on an e-mail.
    #[test]
    fn a_provider_without_its_credential_is_refused() {
        let settings = production_with(&[("email.resend.api_key", "")]);
        let found = problems(&settings, super::Environment::Production);

        assert!(
            found.iter().any(|p| p.contains("email.resend.api_key")),
            "an empty Resend key was accepted: {found:?}"
        );
    }

    #[test]
    fn cloudflare_needs_all_three_of_its_fields() {
        let settings = production_with(&[
            ("email.backend", "cloudflare"),
            ("email.cloudflare.api_token", ""),
            ("email.cloudflare.account_id", ""),
            ("email.cloudflare.from", ""),
        ]);
        let found = problems(&settings, super::Environment::Production);

        for field in ["api_token", "account_id", "from"] {
            assert!(
                found.iter().any(|p| p.contains(field)),
                "cloudflare.{field} was accepted empty: {found:?}"
            );
        }
    }

    /// Selecting one provider must not demand the other's credentials — the whole
    /// point of keeping provider blocks out of the `.yaml`.
    #[test]
    fn choosing_cloudflare_does_not_require_resend() {
        let settings = production_with(&[
            ("email.backend", "cloudflare"),
            ("email.cloudflare.api_token", "placeholder"),
            ("email.cloudflare.account_id", "abc123"),
            ("email.cloudflare.from", "oxid <no-reply@oxid.uk>"),
            ("email.resend.api_key", ""),
        ]);

        settings
            .validate(super::Environment::Production)
            .expect("cloudflare must not be blocked by an unused Resend key");
    }

    /// Confirmation on with no provider creates accounts nobody can confirm.
    #[test]
    fn confirmation_without_a_provider_is_refused_in_production() {
        let settings = production_with(&[("email.backend", "off")]);
        let found = problems(&settings, super::Environment::Production);

        assert!(
            found.iter().any(|p| p.contains("nobody can confirm")),
            "a production deploy that cannot confirm anyone was accepted: {found:?}"
        );
    }

    /// The same configuration is fine on a laptop, and that exception is the
    /// reason the disabled mailer logs the whole message: the link is right there
    /// in the terminal, so the flow stays completable by hand.
    #[test]
    fn confirmation_without_a_provider_is_fine_locally() {
        let settings = assemble("local", &[]).expect("must deserialize");

        assert_eq!(settings.email.backend, super::EmailBackend::Off);
        assert!(settings.email.require_confirmation);

        settings
            .validate(super::Environment::Local)
            .expect("local must stay usable without a mail provider");
    }

    /// `site_url` ends up inside every confirmation link. Scheme-less, the link is
    /// unclickable and the typo surfaces in somebody else's inbox.
    #[test]
    fn a_link_target_without_a_scheme_is_refused() {
        let settings = production_with(&[("email.site_url", "oxid.uk")]);
        let found = problems(&settings, super::Environment::Production);

        assert!(
            found.iter().any(|p| p.contains("email.site_url")),
            "a scheme-less site_url was accepted: {found:?}"
        );
    }

    /// One boot should cost one round of fixes, not one fix per boot.
    #[test]
    fn every_problem_is_reported_at_once() {
        let settings = production_with(&[
            ("email.site_url", "oxid.uk"),
            ("email.resend.api_key", ""),
            ("database.max_connections", "0"),
        ]);
        let found = problems(&settings, super::Environment::Production);

        assert!(
            found.len() >= 3,
            "the validator stopped at the first problem: {found:?}"
        );
    }

    /// The message a person reads when the boot refuses. Worth pinning: it is
    /// the entire diagnosis, delivered once, in a container that then exits.
    #[test]
    fn the_refusal_lists_every_problem_by_name() {
        let settings = production_with(&[("email.site_url", ""), ("email.resend.from", "")]);
        let printed = settings
            .validate(super::Environment::Production)
            .expect_err("must be refused")
            .to_string();

        assert!(printed.contains("email.site_url"), "{printed}");
        assert!(printed.contains("email.resend.from"), "{printed}");
        assert!(printed.contains("cannot be used"), "{printed}");
    }

    /// Each trade-off says something different and says something at all. An empty
    /// or duplicated message is a warning that scrolls past unread.
    #[test]
    fn each_tradeoff_reads_differently() {
        let enumeration = super::Tradeoff::EnumerationOpen.message();
        let silent = super::Tradeoff::NobodyReceivesMail.message();

        assert!(!enumeration.is_empty());
        assert!(!silent.is_empty());
        assert_ne!(enumeration, silent);
    }

    /// A laptop is not a deployment. Warning about the normal way to work on one
    /// trains everybody to ignore the warning that matters.
    #[test]
    fn nothing_is_flagged_outside_production() {
        let settings = assemble("local", &[]).expect("local must load");

        assert!(settings.tradeoffs(super::Environment::Local).is_empty());
    }

    /// The two things a production deploy can give up, each named once.
    #[test]
    fn production_names_what_it_gave_up() {
        let mut overrides: Vec<(&str, &str)> = CLUSTER_ENV.into_iter().collect();
        overrides.push(("email.require_confirmation", "false"));

        let settings = assemble("production", &overrides).expect("must load");

        assert_eq!(
            settings.tradeoffs(super::Environment::Production),
            vec![super::Tradeoff::EnumerationOpen],
            "turning confirmation off in production has to be said out loud"
        );
    }

    #[test]
    fn a_production_deploy_that_sends_nothing_says_so() {
        let mut overrides: Vec<(&str, &str)> = CLUSTER_ENV.into_iter().collect();
        overrides.push(("email.backend", "off"));

        let settings = assemble("production", &overrides).expect("must load");

        assert_eq!(
            settings.tradeoffs(super::Environment::Production),
            vec![super::Tradeoff::NobodyReceivesMail]
        );
    }

    /// The configuration everything actually ships with gives nothing up.
    #[test]
    fn the_shipped_production_configuration_is_clean() {
        let settings = assemble("production", &CLUSTER_ENV).expect("must load");

        assert!(
            settings
                .tradeoffs(super::Environment::Production)
                .is_empty(),
            "production is trading something away without meaning to"
        );
    }

    /// What the migration Job does, built the way the Job is actually built.
    ///
    /// **On top of `CLUSTER_ENV`, not instead of it.** The Job pulls the same
    /// `oxid-config` map the API does, through `envFrom`, and then overrides
    /// a few keys with its own `env`. Testing the overrides alone would have
    /// missed exactly the failure this test exists for: that map now carries
    /// `APP_EMAIL__RESEND__FROM`, which creates a partial provider block and makes
    /// the credential required for a process that sends nothing.
    #[test]
    fn the_migration_jobs_overrides_are_enough_to_boot() {
        let mut overrides: Vec<(&str, &str)> = CLUSTER_ENV.into_iter().collect();
        overrides.extend([
            ("analytics.backend", "off"),
            ("analytics.clickhouse.password", ""),
            ("email.backend", "off"),
            ("email.resend.api_key", ""),
        ]);

        let settings = assemble("production", &overrides)
            .expect("the migrator must boot with no credentials it cannot use");

        assert_eq!(settings.email.backend, super::EmailBackend::Off);
        assert_eq!(settings.analytics.backend, super::AnalyticsBackend::Off);
    }
}
