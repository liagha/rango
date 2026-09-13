# rango

A Django-like web framework for Rust.

Models, routing, templates, authentication, an admin panel, and a CLI for
scaffolding projects and managing the database — wired together with a typed,
transaction-safe store that runs on SQLite or PostgreSQL.

This is the facade crate, published on crates.io as `rango-web` but imported as
`rango`. It re-exports the sub-crates — `rango-core`, `rango-store`,
`rango-admin`, `rango-authentication` — behind one import.

See the [workspace README](https://github.com/liagha/rango) for the full
guide, or [docs.rs/rango-web](https://docs.rs/rango-web) for the API reference.