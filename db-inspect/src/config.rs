/// Configuration for the db-inspect server.
///
/// Values are sourced from environment variables and/or CLI arguments,
/// with sensible defaults when neither is provided.
use std::env;

/// Parsed configuration combining environment variables and CLI flags.
#[derive(Debug, Clone)]
pub struct DbConfig {
    /// Absolute or relative path to the Seneschal SQLite database.
    pub db_path: String,
    /// Port the HTTP server binds to.
    pub port: u16,
    /// Interface the HTTP server binds to (hardcoded `127.0.0.1`).
    pub bind_addr: String,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            db_path: "../data/seneschal.db".into(),
            port: 3000,
            bind_addr: "0.0.0.0".into(),
        }
    }
}

impl DbConfig {
    /// Build configuration from environment variables, then override with CLI values.
    ///
    /// Precedence (highest → lowest):
    /// 1. CLI arguments (passed via `db_path` parameter)
    /// 2. `SENECHAL_DB_PATH` / `DB_INSPECT_PORT` environment variables
    /// 3. Hardcoded defaults
    pub fn from_args(db_path: Option<String>) -> Self {
        let mut config = Self::default();

        // Layer 1: environment variables
        if let Ok(path) = env::var("SENECHAL_DB_PATH") {
            config.db_path = path;
        }
        if let Ok(port_str) = env::var("DB_INSPECT_PORT")
            && let Ok(port) = port_str.parse::<u16>()
        {
            config.port = port;
        }

        // Layer 2: CLI overrides
        if let Some(cli_path) = db_path {
            config.db_path = cli_path;
        }

        config
    }

    /// Return the full bind address string suitable for `TcpListener::bind`.
    pub fn bind_string(&self) -> String {
        format!("{}:{}", self.bind_addr, self.port)
    }
}
