fn main() {
    let db = format!("{}/rango.sqlite", env!("CARGO_MANIFEST_DIR"));
    let mut args = std::env::args().skip(1).peekable();
    match args.peek().map(String::as_str) {
        None | Some("-h") | Some("--help") => {
            println!("{}", rango_cli::usage());
            return;
        }
        _ => {}
    }
    let command = rango_cli::parse(args).unwrap_or_else(|fail| {
        eprintln!("{fail}");
        std::process::exit(2);
    });
    rango::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let store = rango::store::sqlite::open(&db).await.unwrap();
            match command {
                rango_cli::Command::Migrate { drop } => {
                    match rango_cli::migrate(&store, &helloworld::schema(), drop).await {
                        Ok(count) => println!("migrated {count}"),
                        Err(fail) => {
                            eprintln!("migrate failed: {fail}");
                            std::process::exit(1);
                        }
                    }
                }
                rango_cli::Command::Create(rango_cli::Create::User {
                    username,
                    password,
                    superuser,
                }) => {
                    let username = username.unwrap_or_else(|| rango_cli::prompt("Username: "));
                    let password = password.unwrap_or_else(rango_cli::prompt_password);
                    match rango_auth::User::register(store.clone(), &username, &password, superuser)
                        .await
                    {
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
