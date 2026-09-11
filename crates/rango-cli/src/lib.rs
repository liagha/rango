use std::sync::Arc;

use rango::model::Schema;
use rango_store::{Store, StoreError};

pub enum Command {
    Migrate { drop: bool },
    Create(Create),
}

pub enum Create {
    User {
        username: Option<String>,
        password: Option<String>,
        superuser: bool,
    },
}

#[derive(Debug)]
pub enum Fail {
    Usage(String),
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

pub fn code(fail: &Fail) -> i32 {
    match fail {
        Fail::Usage(_) => 2,
        Fail::Error(_) => 1,
    }
}

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
            Ok(Command::Migrate { drop })
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
                Ok(Command::Create(Create::User {
                    username,
                    password,
                    superuser,
                }))
            }
            Some(other) => Err(Fail::Usage(format!("unknown create target {other}"))),
            None => Err(Fail::Usage(usage().into())),
        },
        Some(other) => Err(Fail::Usage(format!("unknown command {other}"))),
        None => Err(Fail::Usage(usage().into())),
    }
}

pub fn usage() -> &'static str {
    "usage: app [command]\ncommands:\n  migrate [--drop]\n  create user [--username NAME] [--password PASS] [--super]"
}

pub async fn exec(
    store: &Arc<dyn Store>,
    schemas: &[Schema],
    command: Command,
) -> Result<String, Fail> {
    match command {
        Command::Migrate { drop } => migrate(store, schemas, drop)
            .await
            .map(|count| format!("migrated {count}"))
            .map_err(|fail| Fail::Error(format!("migrate failed: {fail}"))),
        Command::Create(Create::User {
            username,
            password,
            superuser,
        }) => {
            let Some(username) = username else {
                return Err(Fail::Usage("create user needs --username NAME".into()));
            };
            let Some(password) = password else {
                return Err(Fail::Usage("create user needs --password PASS".into()));
            };
            match rango_auth::User::register(store.clone(), &username, &password, superuser).await {
                Ok(user) => Ok(format!("created user {}", user.username)),
                Err(fail) => Err(Fail::Error(fail.to_string())),
            }
        }
    }
}

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

pub fn prompt(text: &str) -> String {
    use std::io::Write;
    print!("{text}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    line.trim().to_string()
}

pub fn prompt_password() -> String {
    rpassword::prompt_password("Password: ")
        .unwrap_or_default()
        .trim()
        .to_string()
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
            Command::Migrate { drop: false }
        ));
        assert!(matches!(
            parse(args(&["migrate", "--drop"])).unwrap(),
            Command::Migrate { drop: true }
        ));
        assert!(parse(args(&["migrate", "--bogus"])).is_err());
    }

    #[test]
    fn create() {
        let Command::Create(Create::User {
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
        assert!(parse(args(&["create", "group"])).is_err());
        assert!(parse(args(&["create"])).is_err());
        assert!(parse(args(&["bogus"])).is_err());
        assert!(parse(args(&[])).is_err());
    }
}
