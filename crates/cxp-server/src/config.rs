//! Everything the process cannot infer, read once from the environment.
//!
//! A setting whose wrong value fails silently has no default: a wrong public
//! URL or platform URL does not crash, it advertises an audience nothing
//! accepts. A setting that is merely inconvenient to get wrong (bind address,
//! log format) gets one. A variable that is set but unparseable is an error
//! naming it, never a fallback.

use std::net::SocketAddr;

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
                s.parse::<u32>()
                    .ok()
                    .filter(|n| *n >= 2)
                    .ok_or_else(|| anyhow!("a whole number of at least 2"))
            })?,
            run_migrations: vars.parse("CXP_RUN_MIGRATIONS", "true", bool_value)?,
            bind: vars.parse("CXP_BIND", "0.0.0.0:8080", |s| {
                s.parse()
                    .map_err(|_| anyhow!("a socket address like 0.0.0.0:8080"))
            })?,
            public_url,
            resource_uri,
            platform_url: vars.url("CXP_PLATFORM_URL")?,
            introspection_secret: vars.required("CXP_INTROSPECTION_SECRET")?,
            platform_webhook_secret: vars.required("CXP_PLATFORM_WEBHOOK_SECRET")?,
            extra_allowed_hosts: vars.list("CXP_ALLOWED_HOSTS"),
            allowed_origins: vars.list("CXP_ALLOWED_ORIGINS"),
            enforce_quotas: vars.parse("CXP_ENFORCE_QUOTAS", "false", bool_value)?,
            log_format: vars.parse("CXP_LOG_FORMAT", "text", |s| match s {
                "text" => Ok(LogFormat::Text),
                "json" => Ok(LogFormat::Json),
                _ => Err(anyhow!("text or json")),
            })?,
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
        }
    }
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

    /// [`Self::url`], and safe inside a quoted header parameter: the public
    /// URL is written into `resource_metadata="…"` on every `401`.
    fn public_url(&self, name: &str) -> Result<String> {
        let v = self.url(name)?;
        if v.chars()
            .any(|c| c == '"' || c == '\\' || c.is_whitespace())
        {
            return Err(anyhow!(
                "{name} must not contain quotes, backslashes, or whitespace, got {v:?}"
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
            ("CXP_ALLOWED_ORIGINS", "https://console.example"),
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
        assert_eq!(c.allowed_origins, ["https://console.example"]);
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

    #[test]
    fn booleans_are_strict() {
        assert!(bool_value("TRUE").unwrap());
        assert!(!bool_value("0").unwrap());
        assert!(bool_value("yes-please").is_err());
    }

    #[test]
    fn debug_prints_no_secret() {
        let c = load(&env(&[(
            "DATABASE_URL",
            "postgres://cxp:hunter2@db.internal/otto_cxp",
        )]))
        .unwrap();
        let s = format!("{c:?}");
        for secret in ["otto_rs_abc", "otto_whsec_abc", "hunter2"] {
            assert!(!s.contains(secret), "{secret} leaked: {s}");
        }
        assert!(s.contains("otto-cxp.savvagent.com"), "{s}");
    }
}
