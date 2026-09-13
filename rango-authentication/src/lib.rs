//! Authentication and sessions for Rango: signup, login, users, and auth views.
//!
//! The [`Authentication`] struct configures session cookies, login and expiry
//! rules, and binds account state to requests via axum middleware and the
//! [`Current`] extractor. Sign-in, sign-up, logout and password-change pages
//! become available by enabling the `views` feature, which gates the bundled
//! [`User`] registration and login helpers.
#![warn(missing_docs)]

use std::sync::{Arc, Mutex};

use axum::{
    extract::{FromRequestParts, State},
    http::request::Parts,
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
};
use rango_core::{
    Error, Repository, Store, Value,
    chrono::Utc,
    urls::Routes,
    view::{self, Request},
};

mod session;
mod user;
#[cfg(feature = "views")]
mod views;

use session::{Attempts, Claim, claim, cookie, login_url, verify};
pub use user::User;

/// Session cookie and route-guard configuration for signup and login.
#[derive(Clone)]
pub struct Authentication {
    secret: String,
    login: String,
    name: String,
    days: i64,
    signup: bool,
    max: u32,
    minutes: i64,
    attempts: Arc<Mutex<Attempts>>,
}

/// Authentication configuration shared across requests.
///
/// Holds the signing secret, cookie and login-path settings, and binds a
/// signed session cookie to the current [`User`] when attached to a route.
impl Authentication {
    /// Creates a new config with the given signing secret and defaults.
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            login: "/login".into(),
            name: "session".into(),
            days: 14,
            signup: false,
            max: 5,
            minutes: 15,
            attempts: Arc::new(Mutex::new(Attempts::new(5, 15))),
        }
    }

    /// Sets the login path redirect target.
    pub fn login(mut self, path: impl Into<String>) -> Self {
        self.login = path.into();
        self
    }

    /// Sets the session cookie name.
    pub fn cookie(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Sets the session lifetime in days.
    pub fn expiry(mut self, days: i64) -> Self {
        self.days = days;
        self
    }

    /// Enables the registration page.
    pub fn signup(mut self, on: bool) -> Self {
        self.signup = on;
        self
    }

    /// Sets the failed-login attempts before lockout.
    pub fn attempts(mut self, max: u32) -> Self {
        self.max = max;
        self.attempts = Arc::new(Mutex::new(Attempts::new(max, self.minutes)));
        self
    }

    /// Sets the lockout window in minutes.
    pub fn lockout(mut self, minutes: i64) -> Self {
        self.minutes = minutes;
        self.attempts = Arc::new(Mutex::new(Attempts::new(self.max, minutes)));
        self
    }

    /// Attaches session loading to the given routes.
    pub fn session(&self, routes: Routes) -> Routes {
        routes.layer(from_fn_with_state(self.clone(), Self::load))
    }

    /// Guards the given routes, redirecting to the login page when unsigned.
    pub fn require_login(&self, routes: Routes) -> Routes {
        routes.layer(from_fn_with_state(self.clone(), Self::deny))
    }

    /// Guards the given routes, allowing only superusers.
    pub fn require_superuser(&self, routes: Routes) -> Routes {
        routes.layer(from_fn_with_state(self.clone(), Self::deny_super))
    }

    async fn load(State(auth): State<Authentication>, mut req: Request, next: Next) -> Response {
        auth.fill(&mut req).await;
        next.run(req).await
    }

    async fn deny(State(auth): State<Authentication>, mut req: Request, next: Next) -> Response {
        auth.fill(&mut req).await;
        let inside = req
            .extensions()
            .get::<Current>()
            .is_some_and(|current| current.0.is_some());
        if inside {
            next.run(req).await
        } else {
            view::redirect(&login_url(&auth.login, &req))
        }
    }

    async fn deny_super(
        State(auth): State<Authentication>,
        mut req: Request,
        next: Next,
    ) -> Response {
        auth.fill(&mut req).await;
        match req.extensions().get::<Current>() {
            Some(Current(Some(user))) if user.superuser => next.run(req).await,
            Some(Current(Some(_))) => Error::Forbidden.into_response(),
            _ => view::redirect(&login_url(&auth.login, &req)),
        }
    }

    async fn fill(&self, req: &mut Request) {
        if req.extensions().get::<Current>().is_none() {
            let (raw, store) = self.peek(req);
            let user = self.who(raw, store).await;
            req.extensions_mut().insert(Current(user));
        }
    }

    fn peek(&self, req: &Request) -> (Option<Claim>, Option<Arc<dyn Store>>) {
        let raw = cookie(req.headers(), &self.name).and_then(|raw| claim(&raw));
        let store = req.extensions().get::<Arc<dyn Store>>().cloned();
        (raw, store)
    }

    async fn who(&self, raw: Option<Claim>, store: Option<Arc<dyn Store>>) -> Option<User> {
        let (id, exp, sig) = raw?;
        let store = store?;
        if exp < Utc::now().timestamp() {
            return None;
        }
        if !verify(&self.secret, id, exp, &sig) {
            return None;
        }
        match Repository::<User>::new(store).get(&Value::int(id)).await {
            Ok(user) => user,
            Err(failed) => {
                tracing::warn!(error = %failed, "session lookup failed");
                None
            }
        }
    }
}

#[derive(Clone)]
/// The currently signed-in user carried in request extensions.
///
/// `None` when no valid session cookie was provided. Extractable from any
/// handler once the session middleware has run.
pub struct Current(pub Option<User>);

impl FromRequestParts<()> for Current {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, _: &()) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<Current>()
            .cloned()
            .unwrap_or(Current(None)))
    }
}
