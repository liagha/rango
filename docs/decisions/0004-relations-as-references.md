# 4. Relations as references in the schema

Status: Accepted

## Context

Models relate: a message has an author, a product has categories. Relying on
raw id columns is stringly, and a runtime "background jobs" style loader
would miss the point. Relations must be declared data so the framework can
render them.

## Decision

A foreign key is `Field::references(target)` — the field names the table it
points at, discovered from the schema, never from runtime config.

Many-to-many is `Field::link(Link::Via { through, theirs })`. `through`
names the join table, `theirs` names the column on the other side. The join
table is framework-owned: the admin syncs it row-by-row (`remove` the missing,
`create` the chosen) instead of growing the `Store` API (see ADR 3).

`Repository::related` walks either direction from the schema, so a reverse
relation is just the counterpart field declared on the other model. `unique`
and `indexed` pass through to the backend.

## Consequences

- The admin renders FK as a single-select, M2M as a multi-select, with the
  choice list derived from the target model.
- Through rows are state the framework manages, not user-modelled state.
- Relations stay introspectable: one field declares its whole shape.