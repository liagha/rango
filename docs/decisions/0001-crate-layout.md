# 1. Crate layout and the facade

Status: Accepted

## Context

rango started as a monolith. A single crate mixed the engine, the store,
admin, auth, and the CLI. Compile times and testing suffered, and nothing
forced a boundary between "framework" and "app".

## Decision

Split into single-purpose crates in one workspace, layered strictly outward:

- `rango-core` — engine: `Error`, `App`, `Settings`, `urls::Routes`,
  `view::View`, `model::Repository`, schema types, forgery, `prelude`.
- `rango-store` — value codec (`Value`, `Writer`/`Reader`, `Storable`,
  `Show`) and one `Store` trait, implemented by `sqlite` and `postgres`.
- `rango-macros` — `Model` derive, `#[rango::template]`, `#[rango::main]`.
- `rango-authentication` — sessions, users, guards.
- `rango-admin` — schema-driven CRUD, history ledger, deeds.
- `rango` — facade: re-exports `rango_core::*`, `store`, `admin`,
  `authentication`; owns the CLI and the `skel` project scaffold.
- `examples/helloworld` — the reference demo app.

Dependencies point inward only: store ← core ← macros ← auth/admin ← facade.

## Consequences

- The engine never imports admin or auth; each crate compiles and tests alone.
- `rango::*` is the single public import point for app authors.
- The facade can run dual-duty (server and CLI) because store + schemas live
  under it.
- Adding a backend or a guard touches one crate.