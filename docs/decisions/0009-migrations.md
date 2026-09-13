# 9. Versioned, forward-only migrations

Status: Accepted

## Context

Today the schema is synced eagerly: `Store::evolve` diffs each table against
its `Schema` and applies the statements, so the DB always matches the models
(and `Db::Migrate { drop }` drops the surplus columns). That is fine for
development but wrong for shipped data: evolution must be explicit, ordered,
recorded, and never backwards.

## Decision

Introduce forward-only, versioned SQL migrations:

- Migration files live in `migrations/`, named `NNNN_description.sql` in
  ascending order of `NNNN`.
- A `_migrations` table records each applied file (name, checksum, applied
  at).
- `rango migrate make <description>` scaffolds the next-numbered file;
  `rango migrate run` applies every unapplied file in order, inside a
  transaction, recording the checksum; `rango migrate status` reports
  applied/pending.
- A file whose checksum no longer matches its record is a hard error —
  applied files are immutable. Rollback is not permitted: repairs are forward
  migrations.
- `evolve` stays as the dev-time convenience for greenfield projects and is
  not used once `migrations/` exists.

## Consequences

- Deploys are deterministic and auditable, matching the framework's rule that
  the store owns persistent state.
- Schema drift is a statement a human reviewed, not a runtime diff.
- The CLI diff surface (`Command::Db`) gains the migration commands without
  new tools.