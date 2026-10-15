//! Everything the process cannot infer, read once from the environment.
//!
//! A setting whose wrong value fails silently has no default: a wrong public
//! URL or platform URL does not crash, it advertises an audience nothing
//! accepts. A setting that is merely inconvenient to get wrong (bind address,
//! log format) gets one. A variable that is set but unparseable is an error
//! naming it, never a fallback.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub db_max_connections: u32,
    pub run_migrations: bool,
    pub bind: SocketAddr,
    /// Public origin, e.g. `https://otto-cxp.savvagent.com`, with no trailing
    /// slash. It lands inside a quoted `WWW-Authenticate` parameter, so it
    /// never contains `"`, `\`, or whitespace.
    pub public_url: String,
    /// Token audience; exactly the `resource_uri` registered at the platform.
    /// Defaults to `{public_url}/mcp`.
    pub resource_uri: String,
    pub platform_url: String,
    /// `otto_rs_…`: this service's credential at the platform.
    pub introspection_secret: String,
    /// `otto_whsec_…`: verifies the platform's lifecycle webhooks.
    pub platform_webhook_secret: String,
    /// `Host` headers `/mcp` accepts besides the public URL's own host.
    pub extra_allowed_hosts: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub enforce_quotas: bool,
    pub log_format: LogFormat,
    /// The web console's sign-in, or `None` while `CXP_CONSOLE_CLIENT_ID` is
    /// unset: then `/auth/login` answers `503 console_login_disabled` and the
    /// session cookie is no credential anywhere.
    pub console: Option<ConsoleConfig>,
    /// `CXP_CONSOLE_DIR`: the built web console, served with an `index.html`
    /// fallback for browser navigations. Without an `index.html` there, every
    /// unknown path is the JSON `404`.
    pub console_dir: PathBuf,
}

/// The web console's sign-in settings.
#[derive(Clone)]
pub struct ConsoleConfig {
    /// `CXP_CONSOLE_CLIENT_ID`: the first-party public client registered at
    /// the platform with the redirect URI `{public_url}/auth/callback`. Not a
    /// secret.
    pub client_id: String,
    /// `CXP_ENCRYPTION_KEY`: base64 of 32 bytes; seals the session tokens at
    /// rest and the in-flight sign-in cookie. Required with a client id.
    pub encryption_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Text,
    Json,
}

/// Written by hand so a `{:?}` can never print a secret.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("db_max_connections", &self.db_max_connections)
            .field("run_migrations", &self.run_migrations)
            .field("bind", &self.bind)
            .field("public_url", &self.public_url)
            .field("resource_uri", &self.resource_uri)
            .field("platform_url", &self.platform_url)
            .field("introspection_secret", &"<redacted>")
            .field("platform_webhook_secret", &"<redacted>")
            .field("extra_allowed_hosts", &self.extra_allowed_hosts)
            .field("allowed_origins", &self.allowed_origins)
            .field("enforce_quotas", &self.enforce_quotas)
            .field("log_format", &self.log_format)
            .field(
                "console_client_id",
                &self.console.as_ref().map(|c| c.client_id.as_str()),
            )
            .field(
                "encryption_key",
                &self.console.as_ref().map(|_| "<redacted>"),
            )
            .field("console_dir", &self.console_dir)
            // The database URL carries a password.
            .finish_non_exhaustive()
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// [`Self::from_env`] over any source of variables, so the parsing is
    /// testable without touching the process environment.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let vars = Vars(&get);
        let public_url = vars.public_url("CXP_PUBLIC_URL")?;
        let resource_uri = match vars.optional("CXP_RESOURCE_URI") {
            Some(v) => {
                http_url("CXP_RESOURCE_URI", &v)?;
                v
            }
            None => format!("{public_url}/mcp"),
        };
        let console = console_config(&vars, &public_url)?;
        Ok(Self {
            // `fly postgres attach` sets DATABASE_URL; CXP_DATABASE_URL wins
            // so a shell with another service's DATABASE_URL cannot point this
            // one at the wrong database by accident.
            database_url: vars
                .optional("CXP_DATABASE_URL")
                .or_else(|| vars.optional("DATABASE_URL"))
                .ok_or_else(|| {
                    anyhow!("CXP_DATABASE_URL (or DATABASE_URL) is required and not set")
                })?,
            db_max_connections: vars.parse("CXP_DB_MAX_CONNECTIONS", "5", |s| {
                // The production database is small and shared by every
                // otto service: a typo here must not take its connections.
                s.parse::<u32>()
                    .ok()
                    .filter(|n| (MIN_CONNECTIONS..=MAX_CONNECTIONS).contains(n))
                    .ok_or_else(|| {
                        anyhow!("a whole number from {MIN_CONNECTIONS} to {MAX_CONNECTIONS}")
                    })
            })?,
            run_migrations: vars.parse("CXP_RUN_MIGRATIONS", "true", bool_value)?,
            bind: vars.parse("CXP_BIND", "0.0.0.0:8080", |s| {
                s.parse()
                    .map_err(|_| anyhow!("a socket address like 0.0.0.0:8080"))
            })?,
            public_url,
            resource_uri,
            platform_url: vars.platform_url("CXP_PLATFORM_URL")?,
            introspection_secret: vars.required("CXP_INTROSPECTION_SECRET")?,
            platform_webhook_secret: vars.required("CXP_PLATFORM_WEBHOOK_SECRET")?,
            extra_allowed_hosts: vars
                .list("CXP_ALLOWED_HOSTS")
                .iter()
                .map(|h| host_entry("CXP_ALLOWED_HOSTS", h))
                .collect::<Result<_>>()?,
            allowed_origins: vars
                .list("CXP_ALLOWED_ORIGINS")
                .iter()
                .map(|o| origin_entry("CXP_ALLOWED_ORIGINS", o))
                .collect::<Result<_>>()?,
            enforce_quotas: vars.parse("CXP_ENFORCE_QUOTAS", "false", bool_value)?,
            log_format: vars.parse("CXP_LOG_FORMAT", "text", |s| match s {
                "text" => Ok(LogFormat::Text),
                "json" => Ok(LogFormat::Json),
                _ => Err(anyhow!("text or json")),
            })?,
            console,
            // Relative to the working directory: right for a `cargo run` from
            // the repository root. The image sets /srv/console.
            console_dir: vars
                .optional("CXP_CONSOLE_DIR")
                .unwrap_or_else(|| "apps/web/dist".into())
                .into(),
        })
    }

    /// Where an org upgrades its plan, named in quota errors: the platform's
    /// billing settings.
    pub fn upgrade_url(&self) -> String {
        format!(
            "{}/settings/billing",
            self.platform_url.trim_end_matches('/')
        )
    }

    /// The `Host` headers `/mcp` accepts: the public URL's host (with its
    /// port when that is not the scheme's default), then any extras.
    ///
    /// Never empty, because rmcp reads an empty list as "allow any host":
    /// [`Self::from_lookup`] refuses a public URL without a host, and
    /// [`Self::for_test`] has one.
    pub fn allowed_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = url::Url::parse(&self.public_url)
            .ok()
            .and_then(|u| {
                let host = u.host_str()?.to_string();
                Some(match u.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host,
                })
            })
            .into_iter()
            .collect();
        for extra in &self.extra_allowed_hosts {
            if !hosts.contains(extra) {
                hosts.push(extra.clone());
            }
        }
        hosts
    }

    /// A complete configuration with placeholder values, for tests.
    #[doc(hidden)]
    pub fn for_test() -> Self {
        Self {
            database_url: "postgres://localhost/test".into(),
            db_max_connections: 5,
            run_migrations: true,
            bind: "127.0.0.1:0".parse().unwrap(),
            public_url: "https://cxp.example.com".into(),
            resource_uri: "https://cxp.example.com/mcp".into(),
            platform_url: "https://platform.example.com".into(),
            introspection_secret: "otto_rs_test".into(),
            platform_webhook_secret: "otto_whsec_test".into(),
            extra_allowed_hosts: vec![],
            allowed_origins: vec![],
            enforce_quotas: false,
            log_format: LogFormat::Text,
            console: None,
            console_dir: "apps/web/dist".into(),
        }
    }
}

/// The console's sign-in, read only when `CXP_CONSOLE_CLIENT_ID` is set, so a
/// deployment that has not registered the client (or set the key) boots with
/// it off and everything else unchanged. A key that is set is checked either
/// way: a typo should not wait until the day the console is switched on.
fn console_config<F: Fn(&str) -> Option<String>>(
    vars: &Vars<'_, F>,
    public_url: &str,
) -> Result<Option<ConsoleConfig>> {
    let key = vars.optional("CXP_ENCRYPTION_KEY");
    if let Some(key) = &key {
        otto_tenant::crypto::Cipher::from_base64_key(key).map_err(|_| {
            anyhow!(
                "CXP_ENCRYPTION_KEY must be base64 of exactly 32 bytes \
                 (generate one with `openssl rand -base64 32`)"
            )
        })?;
    }
    let Some(client_id) = vars.optional("CXP_CONSOLE_CLIENT_ID") else {
        return Ok(None);
    };
    let encryption_key = key.ok_or_else(|| {
        anyhow!(
            "CXP_ENCRYPTION_KEY is required when CXP_CONSOLE_CLIENT_ID is set: it seals the \
             console's sessions"
        )
    })?;
    // The session cookie is `__Host-` and `Secure` everywhere but a loopback
    // development server; a plain-http public URL anywhere else would send it
    // in the clear.
    let parsed = url::Url::parse(public_url)?;
    if parsed.scheme() != "https" && !is_loopback(&parsed) {
        return Err(anyhow!(
            "the console sign-in (CXP_CONSOLE_CLIENT_ID) needs an https CXP_PUBLIC_URL \
             (plain http only for a loopback host), got {public_url:?}"
        ));
    }
    Ok(Some(ConsoleConfig {
        client_id,
        encryption_key,
    }))
}

struct Vars<'a, F: Fn(&str) -> Option<String>>(&'a F);

impl<F: Fn(&str) -> Option<String>> Vars<'_, F> {
    /// Set and not blank, trimmed.
    fn optional(&self, name: &str) -> Option<String> {
        (self.0)(name)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    fn required(&self, name: &str) -> Result<String> {
        self.optional(name)
            .ok_or_else(|| anyhow!("{name} is required and not set"))
    }

    /// An absolute http(s) URL, without its trailing slash.
    fn url(&self, name: &str) -> Result<String> {
        let v = self.required(name)?;
        http_url(name, &v)?;
        Ok(v.trim_end_matches('/').to_string())
    }

    /// [`Self::url`], an origin only (no path, query, fragment, or
    /// credentials: `/mcp` and the metadata document are appended to it),
    /// and safe inside a quoted header parameter: the public URL is written
    /// into `resource_metadata="…"` on every `401`.
    fn public_url(&self, name: &str) -> Result<String> {
        let v = self.url(name)?;
        let parsed = url::Url::parse(&v)?;
        if parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(anyhow!(
                "{name} must be an origin like https://otto-cxp.savvagent.com, with no path, \
                 query, or credentials, got {v:?}"
            ));
        }
        if v.chars()
            .any(|c| c == '"' || c == '\\' || c.is_whitespace())
        {
            return Err(anyhow!(
                "{name} must not contain quotes, backslashes, or whitespace, got {v:?}"
            ));
        }
        Ok(v)
    }

    /// [`Self::url`], and `https` unless the host is loopback: the
    /// introspection secret is sent to it on every call.
    fn platform_url(&self, name: &str) -> Result<String> {
        let v = self.url(name)?;
        let parsed = url::Url::parse(&v)?;
        if parsed.scheme() != "https" && !is_loopback(&parsed) {
            return Err(anyhow!(
                "{name} must use https (plain http only for a loopback host), got {v:?}"
            ));
        }
        Ok(v)
    }

    fn list(&self, name: &str) -> Vec<String> {
        self.optional(name)
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn parse<T>(&self, name: &str, default: &str, parse: impl Fn(&str) -> Result<T>) -> Result<T> {
        let raw = self.optional(name);
        let value = raw.as_deref().unwrap_or(default);
        parse(value).with_context(|| format!("{name}={value:?} is not valid"))
    }
}

/// The pool size bounds `CXP_DB_MAX_CONNECTIONS` accepts.
pub const MIN_CONNECTIONS: u32 = 2;
pub const MAX_CONNECTIONS: u32 = 20;

fn is_loopback(u: &url::Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// One `Host` value `/mcp` accepts: `host` or `host:port`, no scheme, path,
/// or credentials. Lowercased, as rmcp compares it.
fn host_entry(name: &str, v: &str) -> Result<String> {
    let bad = || anyhow!("{name} entries are host or host:port (no scheme or path), got {v:?}");
    if v.contains("://") || v.contains(['/', '@', '"', '\\']) || v.contains(char::is_whitespace) {
        return Err(bad());
    }
    let authority = http::uri::Authority::try_from(v).map_err(|_| bad())?;
    // `host:port` needs a numeric port; a bracketed IPv6 host carries its
    // own colons.
    let after_host = v.rsplit_once(']').map_or(v, |(_, rest)| rest);
    if authority.host().is_empty() || (after_host.contains(':') && authority.port_u16().is_none()) {
        return Err(bad());
    }
    Ok(v.to_ascii_lowercase())
}

/// One browser origin `/mcp` accepts: `scheme://host[:port]`, normalized
/// as a browser sends it (lowercase, default port dropped).
fn origin_entry(name: &str, v: &str) -> Result<String> {
    let bad = || anyhow!("{name} entries are origins like https://app.example.com, got {v:?}");
    let parsed = url::Url::parse(v).map_err(|_| bad())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(bad());
    }
    Ok(parsed.origin().ascii_serialization())
}

fn http_url(name: &str, v: &str) -> Result<()> {
    let parsed = url::Url::parse(v).with_context(|| format!("{name} is not a URL: {v:?}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(anyhow!("{name} must be an absolute http(s) URL, got {v:?}"));
    }
    Ok(())
}

fn bool_value(s: &str) -> Result<bool> {
    match s.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(anyhow!("true or false")),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// The minimum a deployment must set.
    fn env(extra: &[(&str, &str)]) -> HashMap<String, String> {
        let mut vars: HashMap<String, String> = [
            (
                "DATABASE_URL",
                "postgres://cxp:cxp@localhost:15435/otto_cxp",
            ),
            ("CXP_PUBLIC_URL", "https://otto-cxp.savvagent.com/"),
            ("CXP_PLATFORM_URL", "https://otto.savvagent.com"),
            ("CXP_INTROSPECTION_SECRET", "otto_rs_abc"),
            ("CXP_PLATFORM_WEBHOOK_SECRET", "otto_whsec_abc"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        for (k, v) in extra {
            vars.insert(k.to_string(), v.to_string());
        }
        vars
    }

    fn load(vars: &HashMap<String, String>) -> Result<Config> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    fn error(vars: &HashMap<String, String>) -> String {
        format!("{:#}", load(vars).unwrap_err())
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let c = load(&env(&[])).unwrap();
        assert_eq!(c.public_url, "https://otto-cxp.savvagent.com");
        assert_eq!(c.resource_uri, "https://otto-cxp.savvagent.com/mcp");
        assert_eq!(c.platform_url, "https://otto.savvagent.com");
        assert_eq!(
            c.database_url,
            "postgres://cxp:cxp@localhost:15435/otto_cxp"
        );
        assert_eq!(c.db_max_connections, 5);
        assert!(c.run_migrations);
        assert_eq!(c.bind, "0.0.0.0:8080".parse().unwrap());
        assert!(c.extra_allowed_hosts.is_empty() && c.allowed_origins.is_empty());
        assert!(!c.enforce_quotas);
        assert_eq!(c.log_format, LogFormat::Text);
        assert_eq!(
            c.upgrade_url(),
            "https://otto.savvagent.com/settings/billing"
        );
        assert_eq!(c.allowed_hosts(), ["otto-cxp.savvagent.com"]);
    }

    #[test]
    fn every_setting_is_read() {
        let c = load(&env(&[
            ("CXP_DATABASE_URL", "postgres://own/db"),
            ("CXP_RESOURCE_URI", "https://otto-cxp.savvagent.com/mcp/"),
            ("CXP_DB_MAX_CONNECTIONS", "8"),
            ("CXP_RUN_MIGRATIONS", "off"),
            ("CXP_BIND", "127.0.0.1:9000"),
            ("CXP_ALLOWED_HOSTS", " otto-cxp.fly.dev , ,other.example "),
            (
                "CXP_ALLOWED_ORIGINS",
                "https://Console.example:443/, http://localhost:5173",
            ),
            ("CXP_ENFORCE_QUOTAS", "yes"),
            ("CXP_LOG_FORMAT", "json"),
            ("CXP_INTROSPECTION_SECRET", "  otto_rs_padded \n"),
        ]))
        .unwrap();
        assert_eq!(c.database_url, "postgres://own/db", "CXP_ wins");
        // The audience is taken verbatim: it must equal the registered URI.
        assert_eq!(c.resource_uri, "https://otto-cxp.savvagent.com/mcp/");
        assert_eq!(c.db_max_connections, 8);
        assert!(!c.run_migrations);
        assert_eq!(c.bind, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(c.extra_allowed_hosts, ["otto-cxp.fly.dev", "other.example"]);
        assert_eq!(
            c.allowed_origins,
            ["https://console.example", "http://localhost:5173"]
        );
        assert!(c.enforce_quotas);
        assert_eq!(c.log_format, LogFormat::Json);
        assert_eq!(
            c.introspection_secret, "otto_rs_padded",
            "secrets are trimmed"
        );
    }

    #[test]
    fn allowed_hosts_start_with_the_public_host_and_are_never_empty() {
        let mut c = Config::for_test();
        assert_eq!(c.allowed_hosts(), ["cxp.example.com"]);
        c.extra_allowed_hosts = vec!["otto-cxp.fly.dev".into(), "cxp.example.com".into()];
        assert_eq!(c.allowed_hosts(), ["cxp.example.com", "otto-cxp.fly.dev"]);

        // A non-default port is part of the authority; a default one is not.
        let c = load(&env(&[("CXP_PUBLIC_URL", "http://localhost:8080")])).unwrap();
        assert_eq!(c.allowed_hosts(), ["localhost:8080"]);
        assert_eq!(c.resource_uri, "http://localhost:8080/mcp");
        let c = load(&env(&[("CXP_PUBLIC_URL", "https://cxp.example.com:443")])).unwrap();
        assert_eq!(c.allowed_hosts(), ["cxp.example.com"]);
    }

    #[test]
    fn a_bad_public_url_is_refused() {
        for bad in [
            "otto-cxp.savvagent.com",
            "/relative",
            "ftp://otto-cxp.savvagent.com",
            "mailto:ops@example.com",
            "https://otto-cxp.savvagent.com/a\"b",
            "https://otto-cxp.savvagent.com/a\\b",
            "https://otto-cxp.savvagent.com/a b",
            "https://otto-cxp.savvagent.com/a\tb",
        ] {
            let msg = error(&env(&[("CXP_PUBLIC_URL", bad)]));
            assert!(msg.contains("CXP_PUBLIC_URL"), "{bad:?}: {msg}");
        }
        let mut vars = env(&[]);
        vars.remove("CXP_PUBLIC_URL");
        assert!(error(&vars).contains("CXP_PUBLIC_URL is required"));
        let msg = error(&env(&[("CXP_RESOURCE_URI", "not a url")]));
        assert!(msg.contains("CXP_RESOURCE_URI"), "{msg}");
        let msg = error(&env(&[("CXP_PLATFORM_URL", "otto.savvagent.com")]));
        assert!(msg.contains("CXP_PLATFORM_URL"), "{msg}");
    }

    #[test]
    fn the_public_url_is_an_origin_and_the_platform_is_https_off_loopback() {
        for bad in [
            "https://otto-cxp.savvagent.com/app",
            "https://otto-cxp.savvagent.com/?x=1",
            "https://otto-cxp.savvagent.com/#top",
            "https://ops:pw@otto-cxp.savvagent.com",
        ] {
            let msg = error(&env(&[("CXP_PUBLIC_URL", bad)]));
            assert!(msg.contains("CXP_PUBLIC_URL"), "{bad:?}: {msg}");
        }
        let msg = error(&env(&[("CXP_PLATFORM_URL", "http://otto.savvagent.com")]));
        assert!(msg.contains("must use https"), "{msg}");
        for ok in [
            "http://localhost:8090",
            "http://127.0.0.1:8090",
            "http://[::1]:8090",
            "https://otto.savvagent.com",
        ] {
            let c = load(&env(&[("CXP_PLATFORM_URL", ok)])).unwrap();
            assert_eq!(c.platform_url, ok);
        }
    }

    #[test]
    fn allowed_hosts_and_origins_are_validated() {
        for bad in [
            "https://otto-cxp.fly.dev",
            "otto-cxp.fly.dev/mcp",
            "user@otto-cxp.fly.dev",
            "otto-cxp.fly.dev:notaport",
        ] {
            let msg = error(&env(&[("CXP_ALLOWED_HOSTS", bad)]));
            assert!(msg.contains("CXP_ALLOWED_HOSTS"), "{bad:?}: {msg}");
        }
        let c = load(&env(&[(
            "CXP_ALLOWED_HOSTS",
            "Otto-CXP.fly.dev,localhost:8080",
        )]))
        .unwrap();
        assert_eq!(
            c.extra_allowed_hosts,
            ["otto-cxp.fly.dev", "localhost:8080"]
        );

        for bad in [
            "console.example.com",
            "ftp://console.example.com",
            "https://console.example.com/app",
            "https://console.example.com?x",
            "null",
        ] {
            let msg = error(&env(&[("CXP_ALLOWED_ORIGINS", bad)]));
            assert!(msg.contains("CXP_ALLOWED_ORIGINS"), "{bad:?}: {msg}");
        }
    }

    #[test]
    fn an_empty_or_missing_secret_is_refused() {
        for name in ["CXP_INTROSPECTION_SECRET", "CXP_PLATFORM_WEBHOOK_SECRET"] {
            for blank in ["", "   ", "\n"] {
                let msg = error(&env(&[(name, blank)]));
                assert!(msg.contains(&format!("{name} is required")), "{msg}");
            }
            let mut vars = env(&[]);
            vars.remove(name);
            assert!(error(&vars).contains(name));
        }
    }

    #[test]
    fn a_set_but_unparseable_value_is_an_error_naming_it() {
        for (name, bad) in [
            ("CXP_DB_MAX_CONNECTIONS", "1"),
            ("CXP_DB_MAX_CONNECTIONS", "21"),
            ("CXP_DB_MAX_CONNECTIONS", "many"),
            ("CXP_RUN_MIGRATIONS", "maybe"),
            ("CXP_BIND", "8080"),
            ("CXP_ENFORCE_QUOTAS", "yes-please"),
            ("CXP_LOG_FORMAT", "xml"),
        ] {
            let msg = error(&env(&[(name, bad)]));
            assert!(msg.contains(name), "{name}={bad}: {msg}");
        }
        let mut vars = env(&[]);
        vars.remove("DATABASE_URL");
        assert!(error(&vars).contains("CXP_DATABASE_URL (or DATABASE_URL)"));
    }

    const KEY: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";

    #[test]
    fn the_console_is_off_until_its_client_id_is_set_and_needs_the_key_then() {
        let c = load(&env(&[])).unwrap();
        assert!(c.console.is_none());
        assert_eq!(c.console_dir, std::path::Path::new("apps/web/dist"));
        // A key alone does not turn it on.
        let c = load(&env(&[("CXP_ENCRYPTION_KEY", KEY)])).unwrap();
        assert!(c.console.is_none());

        let c = load(&env(&[
            ("CXP_CONSOLE_CLIENT_ID", " cxp-console "),
            ("CXP_ENCRYPTION_KEY", KEY),
            ("CXP_CONSOLE_DIR", "/srv/console"),
        ]))
        .unwrap();
        let console = c.console.unwrap();
        assert_eq!(console.client_id, "cxp-console");
        assert_eq!(console.encryption_key, KEY);
        assert_eq!(c.console_dir, std::path::Path::new("/srv/console"));

        let msg = error(&env(&[("CXP_CONSOLE_CLIENT_ID", "cxp-console")]));
        assert!(msg.contains("CXP_ENCRYPTION_KEY is required"), "{msg}");
    }

    #[test]
    fn a_bad_encryption_key_is_refused_whether_or_not_the_console_is_on() {
        for bad in ["not base64!", "c2hvcnQ=", &format!("{KEY}{KEY}")] {
            for extra in [vec![], vec![("CXP_CONSOLE_CLIENT_ID", "cxp-console")]] {
                let mut vars = vec![("CXP_ENCRYPTION_KEY", bad)];
                vars.extend(extra);
                let msg = error(&env(&vars));
                assert!(
                    msg.contains("CXP_ENCRYPTION_KEY must be base64"),
                    "{bad}: {msg}"
                );
                assert!(!msg.contains(bad), "the key is echoed: {msg}");
            }
        }
    }

    #[test]
    fn the_console_needs_https_off_loopback() {
        let with = |public: &str| {
            env(&[
                ("CXP_PUBLIC_URL", public),
                ("CXP_CONSOLE_CLIENT_ID", "cxp-console"),
                ("CXP_ENCRYPTION_KEY", KEY),
            ])
        };
        let msg = error(&with("http://cxp.example.com"));
        assert!(msg.contains("needs an https CXP_PUBLIC_URL"), "{msg}");
        for ok in [
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            "https://cxp.example.com",
        ] {
            assert!(load(&with(ok)).unwrap().console.is_some(), "{ok}");
        }
    }

    #[test]
    fn booleans_are_strict() {
        assert!(bool_value("TRUE").unwrap());
        assert!(!bool_value("0").unwrap());
        assert!(bool_value("yes-please").is_err());
    }

    #[test]
    fn debug_prints_no_secret() {
        let c = load(&env(&[
            (
                "DATABASE_URL",
                "postgres://cxp:hunter2@db.internal/otto_cxp",
            ),
            ("CXP_CONSOLE_CLIENT_ID", "cxp-console"),
            ("CXP_ENCRYPTION_KEY", KEY),
        ]))
        .unwrap();
        let s = format!("{c:?}");
        assert!(s.contains("cxp-console"), "{s}");
        for secret in ["otto_rs_abc", "otto_whsec_abc", "hunter2", KEY] {
            assert!(!s.contains(secret), "{secret} leaked: {s}");
        }
        assert!(s.contains("otto-cxp.savvagent.com"), "{s}");
    }
}
