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

pub fn parse(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut args = args.peekable();
    match args.next().as_deref() {
        Some("migrate") => {
            let mut drop = false;
            for arg in args.by_ref() {
                match arg.as_str() {
                    "--drop" => drop = true,
                    other => return Err(format!("unknown argument {other}")),
                }
            }
            Ok(Command::Migrate { drop })
        }
        Some("create") => match args.next().as_deref() {
            Some("user") => {
                let mut username = None;
                let mut password = None;
                let mut superuser = false;
                for arg in args.by_ref() {
                    match arg.as_str() {
                        "--username" => username = args.next(),
                        "--password" => password = args.next(),
                        "--super" => superuser = true,
                        other => return Err(format!("unknown argument {other}")),
                    }
                }
                Ok(Command::Create(Create::User {
                    username,
                    password,
                    superuser,
                }))
            }
            Some(other) => Err(format!("unknown create target {other}")),
            None => Err(usage().into()),
        },
        Some(other) => Err(format!("unknown command {other}")),
        None => Err(usage().into()),
    }
}

pub fn usage() -> &'static str {
    "usage: rango <command>\ncommands:\n  migrate [--drop]\n  create user [--username NAME] [--password PASS] [--super]"
}

pub async fn migrate(
    store: &Arc<dyn Store>,
    schemas: &[Schema],
    drop: bool,
) -> Result<usize, StoreError> {
    let mut done = 0;
    for schema in schemas {
        store.execute(&schema.ddl(), &[]).await?;
        done += 1;
        match store.columns(schema.table).await {
            Ok(have) => {
                for sql in schema.alter(&have) {
                    store.execute(&sql, &[]).await?;
                    done += 1;
                }
                if drop {
                    for sql in schema.drop(&have) {
                        store.execute(&sql, &[]).await?;
                        done += 1;
                    }
                }
            }
            Err(StoreError::Unsupported(_)) => {}
            Err(fail) => return Err(fail),
        }
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
