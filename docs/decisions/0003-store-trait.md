# 3. One Store trait, portable Value

Status: Accepted

## Context

rango ships SQLite for development and Postgres for deployment. The engine
cannot know their dialect, but both must share semantics for typed cells,
keys, constraints, and evolution.

## Decision

A single async `Store` trait is the SQL capability boundary:

`execute`, `fetch`, `columns`, `scan_query`, `total_query`, `define`,
`create`, `replace`, `upsert`, `remove`, `evolve`, `mass`, `last_id`,
`insert`, `secret`.

`Value` is the portable cell. It travels textually (`Value::literal`) with a
`Column` affinity hint, and each backend decodes rows into typed `Row`
getters. `Writer`/`Reader` keep Rust values out of the driver layer.

Each backend owns its dialect: column SQL from `Column`, booleans as `'t'`/
`'f'` vs native `bool`, quoting rules, and foreign-key violations mapped to
`StoreError::Reference` (`787` in SQLite, `23503` in Postgres).

`evolve(schema, drop)` diffs the live columns against the schema and issues
the `CREATE`/`ADD`/`DROP`/`RENAME` statements; it powers dev-time sync.

`Store::secret` persists the per-install signing secret in a `setting` table,
generating it on first call.

## Consequences

- The engine speaks only `Store`; adding a backend is one impl.
- Dialect knowledge lives with the backend that owns it.
- `StoreError` (Sql / Value / Channel / Io / Reference / Unsupported) is the
  typed failure channel, bridged into `core::Error` at the boundary.