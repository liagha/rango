use helloworld::Message;
use rango::chrono::Utc;
use rango::prelude::*;
use rango_auth::Current;

#[derive(Template)]
#[template(path = "index.html", askama = rango::askama)]
struct Index {
    name: String,
    messages: Vec<Message>,
    user: String,
}

async fn index(repository: Repository<Message>, current: Current) -> Result<Response, Error> {
    let messages = repository.all().await.map_err(Error::from)?;
    render(Index {
        name: "world".to_string(),
        messages,
        user: current.0.map(|user| user.username).unwrap_or_default(),
    })
}

#[derive(Clone)]
struct About;

impl View for About {
    fn call(self, _req: Request) -> Result<Response, Error> {
        Ok(html(
            "<h1>About Rango</h1><p>A Django-like web framework for Rust.</p>",
        ))
    }
}

async fn admin() -> Result<Response, Error> {
    Ok(redirect("/admin/"))
}

#[derive(Deserialize, Default)]
#[serde(crate = "rango::serde")]
struct Contact {
    name: String,
    message: String,
}

impl Valid for Contact {
    fn errors(&self) -> Errors {
        let mut problems = Errors::new();
        if self.name.trim().is_empty() {
            problems.push("name", "Name is required.");
        }
        if self.message.trim().is_empty() {
            problems.push("message", "Message is required.");
        }
        problems
    }
}

#[derive(Template)]
#[template(path = "contact.html", askama = rango::askama)]
struct ContactPage {
    form: Contact,
    errors: Errors,
    token: String,
}

async fn contact(req: Request) -> Result<Response, Error> {
    render(ContactPage {
        form: Contact::default(),
        errors: Errors::new(),
        token: token(&req),
    })
}

async fn contact_post(
    repository: Repository<Message>,
    headers: HeaderMap,
    Form(form): Form<Contact>,
) -> Result<Response, Error> {
    let errors = form.errors();
    if errors.valid() {
        let mut message = Message {
            id: 0,
            name: form.name,
            message: form.message,
            created: Utc::now(),
        };
        repository.save(&mut message).await.map_err(Error::from)?;
        Ok(redirect("/thanks"))
    } else {
        render(ContactPage {
            form,
            errors,
            token: cookie(&headers).unwrap_or_default(),
        })
    }
}

#[derive(Template)]
#[template(path = "thanks.html", askama = rango::askama)]
struct Thanks;

async fn thanks() -> Result<Response, Error> {
    render(Thanks)
}

fn secret() -> String {
    std::env::var("RANGO_SECRET").unwrap_or_else(|_| {
        eprintln!("warning: RANGO_SECRET is not set, using an insecure default");
        "rango-dev-secret".into()
    })
}

fn hint(store: &std::sync::Arc<dyn rango::Store>) {
    let store = store.clone();
    rango::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let empty = Repository::<rango_auth::User>::new(store)
                .all()
                .await
                .unwrap_or_default()
                .is_empty();
            if empty {
                eprintln!("no users yet — run: cargo run --bin manage -- createsuperuser");
            }
        });
}

fn main() {
    let settings = Settings::new()
        .base_dir(env!("CARGO_MANIFEST_DIR"))
        .secret(secret());
    let db = format!("{}/rango.sqlite", env!("CARGO_MANIFEST_DIR"));
    let store = rango::store::sqlite::open(&db).unwrap();
    hint(&store);
    let auth = rango_auth::Auth::new(&settings.secret).signup(true);
    let panel = rango_admin::Admin::new().model::<Message>();
    App::new(settings)
        .store(store)
        .urls(
            auth.session(
                Routes::new()
                    .route("/", get(index))
                    .route("/about", get_view(About))
                    .route("/contact", get(contact).post(contact_post))
                    .route("/thanks", get(thanks))
                    .route("/admin", get(admin)),
            ),
        )
        .urls(auth.routes())
        .mount("/admin/", auth.require_login(panel.routes()))
        .mount_static()
        .serve()
        .unwrap();
}
