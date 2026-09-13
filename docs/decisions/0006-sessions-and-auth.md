# 6. Signed-cookie sessions and the guard stack

Status: Accepted

## Context

Apps need login and authorization without re-implementing crypto. No external
session store should be required in the default deploy.

## Decision

The signing secret is the install secret persisted by `Store::secret` (ADR 3).
Sessions are cookies signed with HMAC-SHA256 and verified on every request
(the `forgery` module, timing-safe in compare).

Guards are extractors over that session:

- `Current` — the signed-in `User`, if any.
- `Authenticated` — requires a session, else `Error::Forbidden`.
- `require_superuser` — layer guarding the admin mount.

Users come from `rango-authentication`: `signup` is off by default, enabled
with `Authentication::signup(true)`. Passwords are hashed by
`User::hash_password` with the install secret and validated by
`User::check_password`; the same policy is reused by every view. Handler
verbs are uniform: GET `show_login`, POST `issue` / `logout`.

## Consequences

- One binary ships auth with no external provider.
- `Current` makes handler signatures honest about authorization.
- The admin panel is protected by construction (superuser layer), not by
  convention.