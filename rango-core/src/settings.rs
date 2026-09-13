//! Runtime settings for a Rango app.

use std::path::PathBuf;

/// Runtime settings for a Rango app.
pub struct Settings {
    /// Host address to bind.
    pub host: String,
    /// Port to listen on.
    pub port: u16,
    /// Show debug panic messages in responses.
    pub debug: bool,
    /// Enable the CSRF guard middleware.
    pub forgery: bool,
    /// Secret used for per-install signing; required before serving.
    pub secret: Option<String>,
    /// Directory served as static assets.
    pub assets: PathBuf,
}

impl Settings {
    /// Default settings for local development.
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            host: "127.0.0.1".to_string(),
            port: 8000,
            debug: true,
            forgery: true,
            secret: None,
            assets: cwd.join("assets"),
        }
    }

    /// Set the host to bind.
    pub fn host(mut self, host: &str) -> Self {
        self.host = host.to_string();
        self
    }

    /// Set the port to listen on.
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Enable or disable the CSRF guard.
    pub fn forgery(mut self, on: bool) -> Self {
        self.forgery = on;
        self
    }

    /// Set the install secret.
    pub fn secret(mut self, secret: impl Into<String>) -> Self {
        self.secret = Some(secret.into());
        self
    }

    /// Set the base directory; assets resolve to its `assets` subdir.
    pub fn base_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.assets = dir.into().join("assets");
        self
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}