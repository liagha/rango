use std::path::PathBuf;

pub struct Settings {
    pub host: String,
    pub port: u16,
    pub debug: bool,
    pub forgery: bool,
    pub secret: Option<String>,
    pub assets: PathBuf,
}

impl Settings {
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

    pub fn host(mut self, host: &str) -> Self {
        self.host = host.to_string();
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn forgery(mut self, on: bool) -> Self {
        self.forgery = on;
        self
    }

    pub fn secret(mut self, secret: impl Into<String>) -> Self {
        self.secret = Some(secret.into());
        self
    }

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
