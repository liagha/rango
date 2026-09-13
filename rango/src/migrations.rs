//! Versioned, forward-only database migrations on disk: `make`, `run`, `status`.

use std::{path::PathBuf, sync::Arc};

use rango_store::{Page, Pending, Only, Query, Store, Tree};
use sha2::{Digest, Sha256};

use crate::cli::Fail;

const DIR: &str = "migrations";

fn dir() -> Result<PathBuf, Fail> {
    std::env::current_dir()
        .map(|cwd| cwd.join(DIR))
        .map_err(|fail| Fail::Error(fail.to_string()))
}

fn slug(description: &str) -> String {
    description
        .chars()
        .flat_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else if ch.is_ascii_whitespace() {
                Some('_')
            } else {
                None
            }
        })
        .collect()
}

/// Writes the next numbered migration file for `description`.
pub fn make(description: &str) -> Result<String, Fail> {
    let slug = slug(description);
    if slug.trim_matches('_').is_empty() {
        return Err(Fail::Usage("migrate make needs a description with letters".into()));
    }
    let dir = dir()?;
    let mut next = 0;
    if dir.exists() {
        for entry in std::fs::read_dir(&dir).map_err(|fail| Fail::Error(fail.to_string()))? {
            let name = entry
                .map_err(|fail| Fail::Error(fail.to_string()))?
                .file_name()
                .to_string_lossy()
                .into_owned();
            if name.ends_with(".sql") && name.len() > 4 {
                next = next.max(name[..4].parse::<usize>().unwrap_or_default());
            }
        }
    } else {
        std::fs::create_dir_all(&dir).map_err(|fail| Fail::Error(fail.to_string()))?;
    }
    next += 1;
    let file = format!("{next:04}_{slug}.sql");
    std::fs::write(dir.join(&file), format!("-- {next:04} {description}\n"))
        .map_err(|fail| Fail::Error(fail.to_string()))?;
    Ok(format!("made {DIR}/{file}"))
}

/// Applies every unapplied migration file in order, recording checksums.
pub async fn run(store: &Arc<dyn Store>) -> Result<String, Fail> {
    let files = list()?;
    if files.is_empty() {
        return Err(Fail::Error(format!("no migrations in {DIR}")));
    }
    let mut pending = Vec::with_capacity(files.len());
    for path in files {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let sql = std::fs::read_to_string(&path).map_err(|fail| Fail::Error(fail.to_string()))?;
        let checksum = format!("{:x}", Sha256::digest(sql.as_bytes()));
        pending.push(Pending { name, sql, checksum });
    }
    let applied = store
        .migrate(&pending)
        .await
        .map_err(|fail| Fail::Error(fail.to_string()))?;
    let plural = if applied == 1 { "migration" } else { "migrations" };
    Ok(format!("ran {applied} {plural}"))
}

/// Lists each migration file and whether it is applied, pending, or changed.
pub async fn status(store: &Arc<dyn Store>) -> Result<String, Fail> {
    let files = list()?;
    if files.is_empty() {
        return Err(Fail::Error(format!("no migrations in {DIR}")));
    }
    let ledger = rango_store::ledger();
    store
        .define(&ledger)
        .await
        .map_err(|fail| Fail::Error(fail.to_string()))?;
    let rows = store
        .scan_query(
            &ledger,
            &Query {
                tree: Tree::And(Vec::new()),
                sort: Vec::new(),
                page: Page::all(),
                only: Only::All,
                mass: None,
            },
        )
        .await
        .map_err(|fail| Fail::Error(fail.to_string()))?;
    let mut recorded = std::collections::HashMap::new();
    for row in rows {
        recorded.insert(
            row.opt_str(0).unwrap_or_default(),
            row.opt_str(1).unwrap_or_default(),
        );
    }
    let mut lines = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let sql = std::fs::read_to_string(&path).map_err(|fail| Fail::Error(fail.to_string()))?;
        let checksum = format!("{:x}", Sha256::digest(sql.as_bytes()));
        let state = match recorded.get(&name) {
            None => "pending",
            Some(recorded) if recorded == &checksum => "applied",
            Some(_) => "changed",
        };
        lines.push(format!("{state:8} {name}"));
    }
    Ok(lines.join("\n"))
}

fn list() -> Result<Vec<PathBuf>, Fail> {
    let dir = dir()?;
    let mut files = Vec::new();
    if dir.exists() {
        for entry in std::fs::read_dir(&dir).map_err(|fail| Fail::Error(fail.to_string()))? {
            let path = entry
                .map_err(|fail| Fail::Error(fail.to_string()))?
                .path();
            if path.extension().map(|ext| ext == "sql").unwrap_or(false) {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}