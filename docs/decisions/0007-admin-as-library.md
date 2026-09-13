# 7. Admin is a library, not a sub-app

Status: Accepted

## Context

Django's admin is the reference UX for schema-driven CRUD. Copying it as a
closed sub-application would make it unreadable and hard to extend. rango
wants the pattern, not the black box.

## Decision

`rango-admin` is a crate exposing routes built from registered schemas.
`Rango::model::<M>()` registers a model and the facade mounts the resulting
panel at `/admin/` behind `require_superuser`.

For every table the panel gets: list (search, filters, sort, pagination),
new/create, detail (with target-model inlines), edit/replace, delete (POST
with a forgery token), and batch deeds. A history ledger records every
mutation per row as an `Event` (`Create`/`Edit`/`Delete`) with user and
timestamp, rendered on the detail page.

Deeds come from `M::actions()`: a fixed set of `Action`s — name, title, a
`run(store, schema, keys)` over selected rows, and a `logged` flag deciding
whether to record an `Event::Delete`. Row-level actions render per row; the
batch form renders as a select.

FK fields render as single-selects and M2M fields as multi-selects, both
fed by `related` (ADR 4); `create`/`replace` sync the through table.

## Consequences

- The admin is ordinary, readable code — extendable per project.
- Form rendering (`form.rs`) is reused by user pages and the panel.
- History gives every table an audit trail for free.