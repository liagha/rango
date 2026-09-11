use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub fn key(dir: impl AsRef<Path>) -> String {
    if let Ok(secret) = std::env::var("RANGO_SECRET")
        && !secret.is_empty()
    {
        return secret;
    }
    let path = dir.as_ref().join(".rango-secret");
    if let Some(secret) = load(&path) {
        return secret;
    }
    save(&path)
}

fn load(path: &Path) -> Option<String> {
    let secret = std::fs::read_to_string(path).ok()?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        None
    } else {
        Some(secret)
    }
}

fn save(path: &Path) -> String {
    let secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    opts.mode(0o600);
    match opts
        .open(path)
        .and_then(|mut file| file.write_all(secret.as_bytes()))
    {
        Ok(()) => tracing::warn!("generated {}", path.display()),
        Err(fail) => tracing::warn!("ephemeral secret: {fail}"),
    }
    secret
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("rango-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".rango-secret");
        let secret = save(&path);
        assert_eq!(secret.len(), 64);
        assert_eq!(load(&path), Some(secret));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
