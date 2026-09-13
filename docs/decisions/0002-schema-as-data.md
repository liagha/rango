# 2. Schema as data, not an ORM

Status: Accepted

## Context

Early rango leaned on dynamic object-relational mapping. That hid the SQL,
made introspection opaque, and fought Rust's type system. The framework needs
one declaration that drives storage, forms, admin, and queries.

## Decision

A model is a compile-time schema: `impl Model` returns a `Schema` (a `Table`
and a `Vec<Field>`). Each `Field` is built from a widget (`cell`, `id`, `key`,
`str`, `check`, `many`…) and refined with `optional`, `choices`, `unique`,
`indexed`, `default`, `references`, `link`, `on_delete`. The macro
(`#[derive(Model)]`) generates `schema()`, `row()`, and `actions()` from the
struct fields.

`row()` returns only persisted cells. A `many` field has no column and no
row slot — it is virtual. When aligning a row against the full field list,
every `many` position is padded with `Null` (`with_id` for model rows,
`align` for scanned rows). Values cross the boundary through a strict codec:
`Storable`/`Show` write into a `Writer` and read from a `Reader`, so backends
never see raw Rust values.

## Consequences

- Forms, admin, and queries derive from the same `Schema` — nothing is
  duplicated and nothing drifts.
- `Schema::moved`/`drop`/`rename` give the migration path (see ADR 9).
- The virtual-many invariant is documented once on the alignment helpers.