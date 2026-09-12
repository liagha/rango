# Getting started

## Config

Add `rango` to a Cargo project (edition 2024):

```toml
[dependencies]
rango = { path = "/path/to/rango" }
tokio = { version = "1.53", features = ["full"] }
```

## Scaffold a project

Any Rango app doubles as a CLI. Scaffold a fresh project from the app binary:

```
your_app create project my-site
cd my-site
cargo run
```

The generated layout:

```
Cargo.toml
src/main.rs       # app entry point
src/lib.rs        # model definitions
templates/        # askama HTML templates
assets/           # static files served under /assets
```

## Models

Define models as plain structs with the `Model` derive:

```rust
use rango::chrono::{DateTime, Utc};
use rango::Model;

#[derive(Clone, Model)]
#[model(table = "posts")]
pub struct Post {
    #[key]
    pub id: i64,
    pub title: String,
    pub body: String,
    pub draft: bool,
    pub published: DateTime<Utc>,
}
```

- `#[key]` marks the primary key. `id: i64` gives an auto-increment key;
  any other field type gives a text primary key.
- `#[model(table = "...")]` sets the table name. The default is the lowercase
  struct name with an `s` suffix (`Post` becomes `posts`).
- Fields map to store types automatically:

| Rust type              | Store type |
|------------------------|------------|
| `i64`, `i32`, `u64`, … | Int        |
| `f64`, `f32`           | Float      |
| `bool`                 | Bool       |
| `String`               | Str        |
| `DateTime<Utc>`        | Moment     |
| `Decimal`              | Decimal    |

Wrap any field in `Option<T>` to make the column nullable.

See the [model reference](model.md) for the full type and attribute list.

## Routes

Handlers are axum functions with extractors injected from the request:

```rust
use rango::prelude::*;

#[derive(Template)]
#[template(path = "index.html", askama = rango::askama)]
struct Index {
    posts: Vec<Post>,
}

async fn list(repository: Repository<Post>) -> Result<Response, Error> {
    let posts = repository.all().await.map_err(Error::from)?;
    render(Index { posts })
}
```

Wire routes and models into the app:

```rust
use rango::Rango;
use rango::prelude::*;

#[tokio::main]
async fn main() -> ExitCode {
    Rango::serve(env!("CARGO_MANIFEST_DIR"))
        .model::<Post>()
        .routes(Routes::new().route("/", get(list)))
        .authentication(|a| a.signup(true))
        .run()
        .await
}
```

`Repository<M>` is also an extractor:
`async fn handler(repository: Repository<Post>) -> Result<Response, Error>`.
It creates the table on first use, so no manual migration is required to start.

## Templates

Templates live in `templates/` next to `CARGO_MANIFEST_DIR`:

```html
{% extends "base.html" %}
{% block content %}
{% for post in posts %}
<h2>{{ post.title }}</h2>
<p>{{ post.body }}</p>
{% endfor %}
{% endblock %}
```

Declare a struct per page and render it:

```rust
#[derive(Template)]
#[template(path = "index.html", askama = rango::askama)]
struct Index {
    posts: Vec<Post>,
}

async fn list(repository: Repository<Post>) -> Result<Response, Error> {
    let posts = repository.all().await.map_err(Error::from)?;
    render(Index { posts })
}
```

## Forms

Deserialize form posts with the `Form` extractor. Add a `Valid` impl for
server-side validation and re-render with errors on failure:

```rust
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

async fn contact_post(
    repository: Repository<Message>,
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
        render(ContactPage { form, errors })
    }
}
```

## JSON API

Return JSON from a handler with `view::json`, which accepts anything
serializable — including `Value`, `Vec<Value>`, `Row`, and `Column` from
the store:

```rust
use rango::prelude::*;

async fn post_json(repository: Repository<Post>) -> Result<Response, Error> {
    let Some(post) = repository.get(&Value::int(1)).await.map_err(Error::from)? else {
        return Ok(json(Value::Null));
    };
    Ok(json(post.row()))
}
```

`post.row()` is the model's values produced by the `Model` derive; a
`Repository::rows(&query)` call returning `Vec<Row>` serializes the same way.

Read a JSON body with the `Json` extractor:

```rust
use rango::chrono::Utc;
use rango::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "rango::serde")]
struct PostInput {
    title: String,
    body: String,
}

async fn create(Json(input): Json<PostInput>) -> Result<Response, Error> {
    let post = Post {
        id: 0,
        title: input.title,
        body: input.body,
        draft: true,
        published: Utc::now(),
    };
    Ok(json(post.row()))
}
```

`Json<T>` requires `T: Deserialize`, so receive data into a `Deserialize`
struct. In the response, `DateTime` serializes to RFC 3339 and `Decimal` to its
plain string form. The `serde_json` crate is re-exported as `rango::serde_json`
(and `rango::prelude::serde_json`).

## Authentication

The admin panel is mounted at `/admin/` and only superusers may enter it.
Enable open signup, or turn it off and create users from the CLI:

```rust
.authentication(|a| a.signup(true))
```

Read the logged-in user in handlers with the `Current` extractor:

```rust
use rango::authentication::Current;

async fn profile(current: Current) -> Result<Response, Error> {
    let username = current.0.map(|u| u.username).unwrap_or_default();
    render(Profile { username })
}
```

Create the first superuser:

```
your_app create user --username admin --password secret --super
```

Registered models (`.model::<T>()`) appear in the admin panel with search,
list, edit, add, delete, and any custom actions.

## Database

The default store is SQLite. The file lives at `{dir}/store/rango.sqlite`, where
`dir` is the path passed to `Rango::serve()`.

Tables are created on demand by `Repository` and the admin panel. Use the CLI to
create them up front or to alter an existing database:

```
your_app migrate
your_app migrate --drop   # drop all tables first, then recreate
```

### PostgreSQL

Enable the `postgres` feature and connect through `rango_core` directly:

```toml
rango = { ..., features = ["postgres"] }
```

```rust
use std::sync::Arc;

use rango::store::postgres;
use rango::{App, Settings};

async fn boot(routes: Routes) -> Result<(), std::io::Error> {
    let store = postgres::connect("postgres://user:pass@localhost/mydb")
        .await
        .expect("connect to postgres");
    let settings = Settings::new().base_dir(".".as_ref())
        .secret("replace-with-a-long-random-string");
    let mut authentication = rango::authentication::Authentication::new("replace-with-a-long-random-string");
    App::new(settings, store)
        .urls(authentication.session(routes))
        .urls(authentication.routes())
        .mount("/admin/", authentication.require_superuser(panel))
        .run()
        .await
}
```

`postgres::connect` returns the same `Arc<dyn Store>`, so `Repository`, the
admin panel, and transactions behave identically on both backends.

## CLI commands

Run the binary with no arguments to start the server. Any word argument is a command:

| Command                                            | Description                       |
|----------------------------------------------------|-----------------------------------|
| `your_app`                                         | Start the web server              |
| `your_app migrate [--drop]`                        | Create or update tables           |
| `your_app create user [--username NAME] [--password PASS] [--super]` | Create a user   |
| `your_app create project NAME`                     | Scaffold a new project in `./NAME`|
| `your_app --help`                                  | Show this usage                   |

The first run prints a reminder to create a user when the store is empty.