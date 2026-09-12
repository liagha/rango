# Rango

A Django-like web framework for Rust.

Models, routing, templates, authentication, an admin panel, and a CLI for scaffolding
projects and managing the database — wired together with a typed, transaction-safe store
that runs on SQLite or PostgreSQL.

- **Models** — derive `Model` on plain structs, query through a typed `Repository`
- **Store** — SQLite (default) and PostgreSQL backends, with transactions
- **Routes** — handlers with extractors for repository, forms, auth, and request data
- **Templates** — askama with `#[template(path = "...", askama = rango::askama)]`
- **Authentication** — sessions, signup toggle, superuser admin
- **Admin** — auto-generated panel with search, edit, and actions per model
- **CLI** — `create project`, `create user`, `migrate` commands

## Quick start

Scaffold a project (run from any app that uses Rango):

```
your_app create project my-site
cd my-site
cargo run
```

Or depend on `rango` directly and build the app in code:

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

## Documentation

- [Getting started](docs/getting-started.md) — install, project layout, models, routes, templates, authentication, database, CLI
- [Model reference](docs/model.md) — types, fields, queries, keys, repository, store trait, backends, transactions