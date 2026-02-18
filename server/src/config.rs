use std::env;

pub struct Config {
    pub database_url: String,
    pub transport: Transport,
    pub http_host: String,
    pub http_port: u16,
}

pub enum Transport {
    Stdio,
    Http,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();

        let transport = match env::var("TRANSPORT")
            .unwrap_or_else(|_| "stdio".into())
            .to_lowercase()
            .as_str()
        {
            "http" => Transport::Http,
            _ => Transport::Stdio,
        };

        Ok(Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://cxp:cxp_dev@localhost:5432/cxp".into()),
            transport,
            http_host: env::var("HTTP_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
            http_port: env::var("HTTP_PORT")
                .unwrap_or_else(|_| "3000".into())
                .parse()?,
        })
    }
}
