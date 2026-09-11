pub use rango_admin as admin;
pub use rango_authentication as authentication;
pub use rango_core::*;
pub use rango_store as store;

pub mod cli;

use std::{path::PathBuf, process::ExitCode, sync::Arc};

pub struct Rango {
    dir: PathBuf,
    admin: admin::Admin,
    routes: Routes,
    tune: Option<
        Box<dyn FnOnce(authentication::Authentication) -> authentication::Authentication + Send>,
    >,
}

impl Rango {
    pub fn serve(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            admin: admin::Admin::new(),
            routes: Routes::new(),
            tune: None,
        }
    }

    pub fn model<M: Model>(mut self) -> Self {
        self.admin = self.admin.model::<M>();
        self
    }

    pub fn routes(mut self, routes: Routes) -> Self {
        self.routes = self.routes.merge(routes);
        self
    }

    pub fn authentication<F>(mut self, f: F) -> Self
    where
        F: FnOnce(authentication::Authentication) -> authentication::Authentication
            + Send
            + 'static,
    {
        self.tune = Some(Box::new(f));
        self
    }

    pub async fn run(self) -> ExitCode {
        let schemas = schemas(&self.admin);
        let mut argv = std::env::args().skip(1);
        match argv.next() {
            None => {
                let db = self.db().await;
                self.boot(db).await
            }
            Some(word) if word == "-h" || word == "--help" => {
                println!("{}", cli::usage());
                ExitCode::SUCCESS
            }
            Some(word) if word == "migrate" || word == "create" => {
                match cli::parse([word].into_iter().chain(argv)) {
                    Ok(cli::Command::Create(cli::Create::Project { name })) => {
                        match cli::project(&name) {
                            Ok(done) => {
                                println!("{done}");
                                ExitCode::SUCCESS
                            }
                            Err(fail) => {
                                eprintln!("{fail}");
                                ExitCode::from(cli::code(&fail) as u8)
                            }
                        }
                    }
                    Ok(command) => {
                        let db = self.db().await;
                        self.deal(db, schemas, command).await
                    }
                    Err(fail) => {
                        eprintln!("{fail}");
                        ExitCode::from(cli::code(&fail) as u8)
                    }
                }
            }
            Some(word) => {
                eprintln!("unknown command {word}");
                eprintln!("{}", cli::usage());
                ExitCode::from(2)
            }
        }
    }

    async fn db(&self) -> Arc<dyn Store> {
        let store = self.dir.join("store");
        if let Err(fail) = std::fs::create_dir_all(&store) {
            eprintln!("error: {fail}");
            std::process::exit(1);
        }
        match store::sqlite::open(&store.join("rango.sqlite")).await {
            Ok(store) => store,
            Err(fail) => {
                eprintln!("error: {fail}");
                std::process::exit(1);
            }
        }
    }

    async fn deal(
        self,
        store: Arc<dyn Store>,
        schemas: Vec<Schema>,
        command: cli::Command,
    ) -> ExitCode {
        match cli::exec(&store, &schemas, command).await {
            Ok(done) => {
                println!("{done}");
                ExitCode::SUCCESS
            }
            Err(fail) => {
                eprintln!("{fail}");
                ExitCode::from(cli::code(&fail) as u8)
            }
        }
    }

    async fn boot(self, store: Arc<dyn Store>) -> ExitCode {
        hint(&store).await;
        let secret = settings::key(store.as_ref()).await;
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
