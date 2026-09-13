# 8. The facade does double duty: server and CLI

Status: Accepted

## Context

A framework app needs scaffolding, user creation, and schema management. A
second binary or tooling layer would split effort and versions. The app
binary should do it all with a predictable interface.

## Decision

`Rango::new(dir).model::<M>().routes(..).authentication(..)` builds one app.
`run()` decides what to do from the first argument:

- no arguments — bind and serve (`db()` opens `dir/store/rango.sqlite`,
  boot wires `Settings` with the store secret, applies the auth tuning, mounts
  `/admin/`, adds static, and prints a hint when no users exist yet);
- `-h` / `--help` — print `cli::usage()`;
- otherwise — parse a single `Command` and execute it.

Commands form an exhaustive enum so execution has no `unreachable!` arms:
`Command::Project { name }` (scaffold the embedded `skel` into a new
directory) and `Command::Db(Db)` with `Db::Migrate { drop }` /
`Db::Create { username, password, superuser }` (runs against the open store
and the full schema list, users included).

`cli::Fail` owns its exit code: `Usage` → 2, `Error` → 1. `#[rango::main]`
wraps an async entrypoint into a `run()` + exit-code shim.

## Consequences

- One binary: `myapp`, `myapp create user`, `myapp migrate`, `myapp new
  project`.
- The server path and CLI path share the store, schemas, and auth code.
- Scaffolding ships with the framework, so new apps start identical.