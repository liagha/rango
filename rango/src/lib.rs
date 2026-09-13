//! Facade crate of the rango web framework. Re-exports the model, routing,
//! store, admin, and authentication sub-crates behind a single import.
#![warn(missing_docs)]

pub use rango_admin as admin;
pub use rango_authentication as authentication;
pub use rango_core::*;
pub use rango_store as store;

/// Command-line interface for migrations, users, and project scaffolding.
pub mod cli;

use std::{path::PathBuf, process::ExitCode, sync::Arc};

/// Rango application builder, configured then served or run as a CLI.
pub struct Rango {
    dir: PathBuf,
    admin: admin::Admin,
    routes: Routes,
    tune: Option<
        Box<dyn FnOnce(authentication::Authentication) -> authentication::Authentication + Send>,
    >,
}

impl Rango {
    /// New app rooted at the given data directory.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            admin: admin::Admin::new(),
            routes: Routes::new(),
            tune: None,
        }
    }

    /// Register a data model with the admin panel.
    pub fn model<M: Model>(mut self) -> Self {
        self.admin = self.admin.model::<M>();
        self
    }

    /// Add routes to the app.
    pub fn routes(mut self, routes: Routes) -> Self {
        self.routes = self.routes.merge(routes);
        self
    }

    /// Configure the authentication stack before boot.
    pub fn authentication<F>(mut self, f: F) -> Self
    where
        F: FnOnce(authentication::Authentication) -> authentication::Authentication
            + Send
            + 'static,
    {
        self.tune = Some(Box::new(f));
        self
    }

    /// Serve the app, or run a CLI command when arguments are given.
    pub async fn run(self) -> ExitCode {
        let schemas = schemas(&self.admin);
        let mut argv = std::env::args().skip(1);
        match argv.next() {
            None => {
                let store = self.db().await;
                self.boot(store).await
            }
            Some(word) if word == "-h" || word == "--help" => {
                println!("{}", cli::usage());
                ExitCode::SUCCESS
            }
            Some(word) => match cli::parse([word].into_iter().chain(argv)) {
                Ok(cli::Command::Project { name }) => finish(cli::project(&name)),
                Ok(cli::Command::Db(db)) => {
                    let store = self.db().await;
                    finish(cli::exec(&store, &schemas, db).await)
                }
                Err(fail) => finish(Err(fail)),
            },
        }
    }

    async fn db(&self) -> Arc<dyn Store> {
        let store = self.dir.join("store");
        if let Err(fail) = std::fs::create_dir_all(&store) {
            eprintln!("error: {fail}");
            std::process::exit(1);
        }
        match store::Sqlite::open(&store.join("rango.sqlite")).await {
            Ok(store) => store,
            Err(fail) => {
                eprintln!("error: {fail}");
                std::process::exit(1);
            }
        }
    }

    async fn boot(self, store: Arc<dyn Store>) -> ExitCode {
        hint(&store).await;
        let secret = store.secret().await;
        let settings = Settings::new().base_dir(&self.dir).secret(&secret);
        let mut authentication = authentication::Authentication::new(&secret);
        if let Some(tune) = self.tune {
            authentication = tune(authentication);
        }
        let panel = self.admin.routes();
        match App::new(settings, store)
            .urls(authentication.session(self.routes))
            .urls(authentication.routes())
            .mount("/admin/", authentication.require_superuser(panel))
            .mount_static()
            .run()
            .await
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(fail) => {
                eprintln!("error: {fail}");
                ExitCode::FAILURE
            }
        }
    }
}

fn finish(result: Result<String, cli::Fail>) -> ExitCode {
    match result {
        Ok(done) => {
            println!("{done}");
            ExitCode::SUCCESS
        }
        Err(fail) => {
            eprintln!("{fail}");
            fail.exit()
        }
    }
}

fn schemas(admin: &admin::Admin) -> Vec<Schema> {
    let mut all = admin.schemas().to_vec();
    all.push(authentication::User::schema());
    all.push(admin::history());
    all
}

async fn hint(store: &Arc<dyn Store>) {
    let empty = Repository::<authentication::User>::new(store.clone())
        .all()
        .await
        .unwrap_or_default()
        .is_empty();
    if empty {
        let app = std::env::args().next().unwrap_or_else(|| "app".into());
        eprintln!("no users yet — run `{app} create user`");
    }
}
