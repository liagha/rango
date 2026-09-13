use std::{process::ExitCode, sync::Arc};

use rango_core::model::Schema;
use rango_store::{Store, StoreError};

/// A parsed CLI command.
pub enum Command {
    /// A command that runs against the open store.
    Db(Db),
    /// Scaffold a new project directory.
    Project {
        /// Name of the new project directory.
        name: String,
    },
}

/// A command that runs against the open store.
pub enum Db {
    /// Apply schema migrations, dropping tables first when `--drop` is given.
    Migrate {
        /// Drop existing tables before migrating.
        drop: bool,
    },
    /// Create a user with the given credentials.
    Create {
        /// Name for the new user.
        username: Option<String>,
        /// Password for the new user.
        password: Option<String>,
        /// Grant superuser (full admin) access.
        superuser: bool,
    },
}

/// A CLI failure: a usage error or an execution error.
#[derive(Debug)]
pub enum Fail {
    /// Malformed command-line invocation.
    Usage(String),
    /// Failure while executing a command.
    Error(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(msg) | Self::Error(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Fail {}

impl Fail {
    /// Exit code for a failure: 2 for usage errors, 1 for runtime errors.
    pub fn exit(&self) -> ExitCode {
        ExitCode::from(if matches!(self, Self::Usage(_)) { 2 } else { 1 })
    }
}

/// Parse the command line into a command.
#[allow(clippy::while_let_on_iterator)]
pub fn parse(args: impl Iterator<Item = String>) -> Result<Command, Fail> {
    let mut args = args.peekable();
    match args.next().as_deref() {
        Some("migrate") => {
            let mut drop = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--drop" => drop = true,
                    other => return Err(Fail::Usage(format!("unknown argument {other}"))),
                }
            }
            Ok(Command::Db(Db::Migrate { drop }))
        }
        Some("create") => match args.next().as_deref() {
            Some("user") => {
                let mut username = None;
                let mut password = None;
                let mut superuser = false;
                while let Some(arg) = args.next() {
                    match arg.as_str() {
                        "--username" => username = args.next(),
                        "--password" => password = args.next(),
                        "--super" => superuser = true,
                        other => return Err(Fail::Usage(format!("unknown argument {other}"))),
                    }
                }
                Ok(Command::Db(Db::Create {
                    username,
                    password,
                    superuser,
                }))
            }
            Some("project") => {
                let mut name = None;
                for arg in args {
                    if name.is_none() {
                        name = Some(arg);
                    } else {
                        return Err(Fail::Usage(format!("unknown argument {arg}")));
                    }
                }
                Ok(Command::Project {
                    name: name.ok_or_else(|| Fail::Usage("create project needs a NAME".into()))?,
                })
            }
            Some(other) => Err(Fail::Usage(format!("unknown create target {other}"))),
            None => Err(Fail::Usage(usage().into())),
        },
        Some(other) => Err(Fail::Usage(format!("unknown command {other}"))),
        None => Err(Fail::Usage(usage().into())),
    }
}

/// Text shown by `--help` and on unknown commands.
pub fn usage() -> &'static str {
    "usage: app [command]\ncommands:\n  migrate [--drop]\n  create user [--username NAME] [--password PASS] [--super]\n  create project NAME"
}

/// Run a parsed command against the store.
pub async fn exec(
    store: &Arc<dyn Store>,
    schemas: &[Schema],
    db: Db,
) -> Result<String, Fail> {
    match db {
        Db::Migrate { drop } => migrate(store, schemas, drop)
            .await
            .map(|count| format!("migrated {count}"))
            .map_err(|fail| Fail::Error(format!("migrate failed: {fail}"))),
        Db::Create {
            username,
            password,
            superuser,
        } => {
            let Some(username) = username else {
                return Err(Fail::Usage("create user needs --username NAME".into()));
            };
            let Some(password) = password else {
                return Err(Fail::Usage("create user needs --password PASS".into()));
            };
            match rango_authentication::User::register(
                store.clone(),
                &username,
                &password,
                superuser,
            )
            .await
            {
                Ok(user) => Ok(format!("created user {}", user.username)),
                Err(fail) => Err(Fail::Error(fail.to_string())),
            }
        }
    }
}

/// Apply schema migrations to the store, one schema at a time.
pub async fn migrate(
    store: &Arc<dyn Store>,
    schemas: &[Schema],
    drop: bool,
) -> Result<usize, StoreError> {
    let mut done = 0;
    for schema in schemas {
        done += store.evolve(schema, drop).await?;
    }
    Ok(done)
}

const MANIFEST: &str = r#"[package]
name = "{NAME}"
version = "0.1.0"
edition = "2024"

[workspace]

[dependencies]
rango = { path = "{PATH}" }
"#;

/// Scaffold a new project in the current directory.
pub fn project(name: &str) -> Result<String, Fail> {
    if !valid(name) {
        return Err(Fail::Usage(format!(
            "{name} is not a valid project name (letters, digits, dashes, starts with a letter)"
        )));
    }
    let dir = std::env::current_dir()
        .map_err(|fail| Fail::Error(fail.to_string()))?
        .join(name);
    if dir.exists() {
        return Err(Fail::Error(format!("{name} already exists")));
    }
    let crate_name = name.replace('-', "_");
    let files = [
        (
            "Cargo.toml",
            MANIFEST
                .replace("{NAME}", name)
                .replace("{PATH}", env!("CARGO_MANIFEST_DIR")),
        ),
        (".gitignore", include_str!("skel/.gitignore").into()),
        (
            "src/main.rs",
            include_str!("skel/src/main.rs").replace("{CRATE}", &crate_name),
        ),
        ("src/lib.rs", include_str!("skel/src/lib.rs").into()),
        (
            "templates/base.html",
            include_str!("skel/templates/base.html").into(),
        ),
        (
            "templates/index.html",
            include_str!("skel/templates/index.html").into(),
        ),
        (
            "assets/site.css",
            include_str!("skel/assets/site.css").into(),
        ),
    ];
    for (file, body) in files {
        let target = dir.join(file);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|fail| Fail::Error(fail.to_string()))?;
        }
        std::fs::write(target, body).map_err(|fail| Fail::Error(fail.to_string()))?;
    }
    Ok(format!("created project {name} — cd {name} && cargo run"))
}

fn valid(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(raw: &[&str]) -> impl Iterator<Item = String> {
        raw.iter().map(|item| item.to_string())
    }

    #[test]
    fn migrate() {
        assert!(matches!(
            parse(args(&["migrate"])).unwrap(),
            Command::Db(Db::Migrate { drop: false })
        ));
        assert!(matches!(
            parse(args(&["migrate", "--drop"])).unwrap(),
            Command::Db(Db::Migrate { drop: true })
        ));
        assert!(parse(args(&["migrate", "--bogus"])).is_err());
    }

    #[test]
    fn create() {
        let Command::Db(Db::Create {
            username,
            password,
            superuser,
        }) = parse(args(&[
            "create",
            "user",
            "--username",
            "u",
            "--password",
            "p",
            "--super",
        ]))
        .unwrap()
        else {
            panic!("wrong command")
        };
        assert_eq!(username, Some("u".into()));
        assert_eq!(password, Some("p".into()));
        assert!(superuser);
        let Command::Project { name } = parse(args(&["create", "project", "site"])).unwrap() else {
            panic!("wrong command")
        };
        assert_eq!(name, "site");
        assert!(parse(args(&["create", "project"])).is_err());
        assert!(parse(args(&["create", "group"])).is_err());
        assert!(parse(args(&["create"])).is_err());
        assert!(parse(args(&["bogus"])).is_err());
        assert!(parse(args(&[])).is_err());
    }

    #[test]
    fn names() {
        assert!(valid("site"));
        assert!(valid("my_app"));
        assert!(valid("my-app"));
        assert!(!valid("_site"));
        assert!(!valid("9site"));
        assert!(!valid(""));
    }
}
