use rango::model;
use rango::prelude::*;
use rango_auth::Current;

#[derive(Template)]
#[template(path = "index.html", askama = rango::askama)]
struct Index {
    name: String,
    messages: Vec<Message>,
    user: String,
}

#[derive(Clone)]
struct Message {
    id: i64,
    name: String,
    message: String,
    created: i64,
}

impl Model for Message {
    fn table() -> &'static str {
        "messages"
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::id(),
            Field::new("name", Kind::Str),
            Field::new("message", Kind::Str),
            Field::new("created", Kind::DateTime),
        ]
    }

    fn row(&self) -> Vec<Value> {
        vec![
            Value::str(&self.name),
            Value::str(&self.message),
            Value::int(self.created),
        ]
    }

    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(Message {
            id: row.int(0)?,
            name: row.str(1)?,
            message: row.str(2)?,
            created: row.int(3)?,
        })
    }

    fn set_id(&mut self, id: i64) {
        self.id = id;
    }

    fn id(&self) -> i64 {
        self.id
    }
}

async fn index(repo: Repo<Message>, current: Current) -> Result<Response, Error> {
    let messages = repo.all().await.map_err(Error::from)?;
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

async fn kick() -> Result<Response, Error> {
    Ok(redirect("/about"))
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
    repo: Repo<Message>,
    headers: HeaderMap,
    Form(form): Form<Contact>,
) -> Result<Response, Error> {
    let errors = form.errors();
    if errors.valid() {
        let mut message = Message {
            id: 0,
            name: form.name,
            message: form.message,
            created: model::now(),
        };
        repo.save(&mut message).await.map_err(Error::from)?;
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

async fn portal() -> Result<Response, Error> {
    Ok(redirect("/admin/"))
}

fn secret() -> String {
    std::env::var("RANGO_SECRET").unwrap_or_else(|_| {
        eprintln!("warning: RANGO_SECRET is not set, using an insecure default");
        "rango-dev-secret".into()
    })
}

fn seed(store: &std::sync::Arc<dyn rango::Store>) {
    let store = store.clone();
    rango::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let empty = Repo::<rango_auth::User>::new(store.clone())
                .all()
                .await
                .unwrap_or_default()
                .is_empty();
            if empty {
                match rango_auth::User::register(store, "admin", "adminadmin").await {
                    Ok(user) => eprintln!("seeded login {} / adminadmin", user.username),
                    Err(fail) => eprintln!("seed failed: {fail}"),
                }
            }
        });
}

fn main() {
    let settings = Settings::new()
        .base_dir(env!("CARGO_MANIFEST_DIR"))
        .secret(secret());
    let db = format!("{}/rango.sqlite", env!("CARGO_MANIFEST_DIR"));
    let store = rango::store::sqlite::open(&db).unwrap();
    seed(&store);
    let auth = rango_auth::Auth::new(&settings.secret);
    let admin = rango_admin::Admin::new().model::<Message>();
    App::new(settings)
        .store(store)
        .urls(
            auth.session(
                Routes::new()
                    .route("/", get(index))
                    .route("/about", get_view(About))
                    .route("/kick", get(kick))
                    .route("/contact", get(contact).post(contact_post))
                    .route("/thanks", get(thanks))
                    .route("/admin", get(portal)),
            ),
        )
        .urls(auth.routes())
        .mount("/admin/", auth.require_login(admin.routes()))
        .mount_static()
        .serve()
        .unwrap();
}
