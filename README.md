# Rango

[![crates.io](https://img.shields.io/crates/v/rango-web)](https://crates.io/crates/rango-web)
[![docs.rs](https://img.shields.io/docsrs/rango-web)](https://docs.rs/rango-web)
[![license](https://img.shields.io/crates/l/rango-web)](LICENSE)

A Django-like web framework for Rust.

Models, routing, templates, authentication, an admin panel, and a CLI for scaffolding
projects and managing the database — wired together with a typed, transaction-safe store
that runs on SQLite or PostgreSQL.

- **Models** — derive `Model` on plain structs, query through a typed `Repository`
- **Store** — SQLite (default) and PostgreSQL backends, with transactions
- **Routes** — handlers with extractors for repository, forms, auth, and request data
- **Templates** — askama via `#[rango::template(path = "...")]`
- **Authentication** — sessions, signup toggle, superuser admin
- **Admin** — auto-generated panel with search, edit, and actions per model
- **CLI** — `create project`, `create user`, `migrate` commands

## Install

```
cargo add rango-web
```

The import name stays `rango`:

```rust
use rango::Rango;
```

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

#[rango::main]
async fn main() -> ExitCode {
    Rango::new(env!("CARGO_MANIFEST_DIR"))
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
- [API reference](https://docs.rs/rango) — rustdoc for every crate

## Workspace

The framework is split into focused crates, all re-exported through `rango`:

- [`rango-web`](rango) — facade, one import for everything, published as `rango-web`
- [`rango-core`](rango-core) — models, routing, views, forms, request pipeline
- [`rango-store`](rango-store) — typed SQLite/PostgreSQL storage
- [`rango-authentication`](rango-authentication) — signup, login, sessions
- [`rango-admin`](rango-admin) — auto-generated admin panel
- [`rango-macro`](rango-macros) — `Model` derive and attribute macros
- [`examples/helloworld`](examples/helloworld) — minimal runnable app