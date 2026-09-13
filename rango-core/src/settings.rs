//! Runtime settings and the secret key.

use std::path::PathBuf;

use crate::{Column, Store, Value};

/// Persistent secret key for the store, generated on first use.
pub async fn key(store: &dyn Store) -> String {
    if store
        .execute(
            "CREATE TABLE IF NOT EXISTS setting (name TEXT PRIMARY KEY, value TEXT)",
            &[],
        )
        .await
        .is_err()
    {
        return ephemeral("no setting table");
    }
    if let Some(secret) = read(store).await {
        return secret;
    }
    let secret = fresh();
    let _ = store
        .execute(
            "INSERT OR IGNORE INTO setting (name, value) VALUES ('secret', ?)",
            &[Value::str(secret.clone())],
        )
        .await;
    match read(store).await {
        Some(secret) => secret,
        None => ephemeral("no secret row"),
    }
}

async fn read(store: &dyn Store) -> Option<String> {
    let rows = store
        .fetch(
            "SELECT value FROM setting WHERE name = 'secret'",
            &[],
            &[Column::Text],
        )
        .await
        .ok()?;
    match rows.first()?.get(0)? {
        Value::Str(secret) if !secret.is_empty() => Some(secret.clone()),
        _ => None,
    }
}

fn fresh() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn ephemeral(why: &str) -> String {
    tracing::warn!("ephemeral secret: {why}");
    fresh()
}

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

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use crate::store::Sqlite;

    #[tokio::test]
    async fn keeps() {
        let path = std::env::temp_dir().join(format!("rango-secret-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Sqlite::open(&path).await.unwrap();
        let first = key(store.as_ref()).await;
        assert_eq!(first.len(), 64);
        assert_eq!(key(store.as_ref()).await, first);
        let _ = std::fs::remove_file(&path);
    }
}
