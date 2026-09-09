use std::path::PathBuf;

pub struct Settings {
    pub host: String,
    pub port: u16,
    pub debug: bool,
    pub csrf: bool,
    pub secret: String,
    pub static_dir: PathBuf,
}

impl Settings {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            host: "127.0.0.1".to_string(),
            port: 8000,
            debug: true,
            csrf: true,
            secret: "changeme".to_string(),
            static_dir: cwd.join("static"),
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

    pub fn csrf(mut self, on: bool) -> Self {
        self.csrf = on;
        self
    }

    pub fn secret(mut self, secret: impl Into<String>) -> Self {
        self.secret = secret.into();
        self
    }

    pub fn base_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.static_dir = dir.into().join("static");
        self
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}
