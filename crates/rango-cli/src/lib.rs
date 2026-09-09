use std::sync::Arc;

use rango_store::{Store, StoreError};

pub enum Command {
    Migrate,
    CreateSuperuser {
        username: Option<String>,
        password: Option<String>,
    },
}

pub fn parse(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut args = args.peekable();
    match args.next().as_deref() {
        Some("migrate") => Ok(Command::Migrate),
        Some("createsuperuser") => {
            let mut username = None;
            let mut password = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--username" => username = args.next(),
                    "--password" => password = args.next(),
                    other => return Err(format!("unknown argument {other}")),
                }
            }
            Ok(Command::CreateSuperuser { username, password })
        }
        Some(other) => Err(format!("unknown command {other}")),
        None => Err("usage: manage <migrate|createsuperuser>".into()),
    }
}

pub async fn migrate(store: &Arc<dyn Store>, ddls: &[String]) -> Result<usize, StoreError> {
    for ddl in ddls {
        store.execute(ddl, &[]).await?;
    }
    Ok(ddls.len())
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
