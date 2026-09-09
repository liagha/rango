fn main() {
    let db = format!("{}/rango.sqlite", env!("CARGO_MANIFEST_DIR"));
    let store = rango::store::sqlite::open(&db).unwrap();
    let command = rango_cli::parse(std::env::args().skip(1)).unwrap_or_else(|fail| {
        eprintln!("{fail}");
        std::process::exit(2);
    });
    rango::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            match command {
                rango_cli::Command::Migrate => {
                    match rango_cli::migrate(&store, &helloworld::schema()).await {
                        Ok(count) => println!("migrated {count} tables"),
                        Err(fail) => {
                            eprintln!("migrate failed: {fail}");
                            std::process::exit(1);
                        }
                    }
                }
                rango_cli::Command::CreateSuperuser { username, password } => {
                    let username = username.unwrap_or_else(|| rango_cli::prompt("Username: "));
                    let password = password.unwrap_or_else(rango_cli::prompt_password);
                    match rango_auth::User::register(store.clone(), &username, &password).await {
                        Ok(user) => println!("created user {}", user.username),
                        Err(fail) => {
                            eprintln!("error: {fail}");
                            std::process::exit(1);
                        }
                    }
                }
            }
        });
}
