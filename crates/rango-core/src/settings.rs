use std::path::PathBuf;

pub struct Settings {
    pub host: String,
    pub port: u16,
    pub debug: bool,
    pub forgery: bool,
    pub secret: Option<String>,
    pub assets: PathBuf,
    pub api_url: String,
    pub ws_url: String,
    pub ws_port: u16,
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
            api_url: String::new(),
            ws_url: String::new(),
            ws_port: 0,
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

    pub fn api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = url.into();
        self
    }

    pub fn ws_url(mut self, url: impl Into<String>) -> Self {
        self.ws_url = url.into();
        self
    }

    pub fn ws_port(mut self, port: u16) -> Self {
        self.ws_port = port;
        self
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}
