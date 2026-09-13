# 5. Views and the error pipeline

Status: Accepted

## Context

Handlers should be writable and testable without importing axum everywhere,
and templates should be checked at compile time.

## Decision

A handler is a `View`: `fn(Request) -> Result<Response, Error>`. The trait
provides `handler()` turning any view into an axum `MethodRouter`, and
`Request`/`Response` are re-exported aliases so user code never names axum.

Responses are built from helpers: `html`, `json`, `render`, `redirect`. The
`#[rango::template(path = "...")]` attribute derives an askama `Template` out
of a plain data struct, so template typos and missing fields fail the build.

`Error` is the single pipeline error mapped to HTTP: `BadRequest` → 400,
`Forbidden` → 403, `NotFound` → 404, `Server`/`Render` → 500, with `tracing`
on server errors. `From<StoreError>` and `From<askama::Error>` bridge the
boundaries; `?` propagates the whole pipeline.

`App` assembles the router: `urls`, `mount`, `mount_static`, a `NotFound`
fallback, `TraceLayer`, a `CatchPanicLayer` whose body respects
`Settings::debug`, and an optional CSRF guard (`forgery` feature +
`Settings::forgery`) with a body-size limit.

## Consequences

- Handlers stay framework-shaped, not axum-shaped, and are unit-testable.
- Status mapping, logging, and panic handling live exactly once.
- Template structs double as documentation of a page's data.