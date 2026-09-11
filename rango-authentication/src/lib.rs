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

#[derive(Clone)]
pub struct Authentication {
    secret: String,
    login: String,
    cookie: String,
    days: i64,
    signup: bool,
    attempts: Arc<Mutex<Attempts>>,
}

impl Authentication {
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            login: "/login".into(),
            cookie: "session".into(),
            days: 14,
            signup: false,
            attempts: Arc::new(Mutex::new(Attempts::new(5, 15))),
        }
    }

    pub fn login(mut self, path: impl Into<String>) -> Self {
        self.login = path.into();
        self
    }

    pub fn cookie(mut self, name: impl Into<String>) -> Self {
        self.cookie = name.into();
        self
    }

    pub fn expiry(mut self, days: i64) -> Self {
        self.days = days;
        self
    }

    pub fn signup(mut self, on: bool) -> Self {
        self.signup = on;
        self
    }

    pub fn attempts(self, max: u32) -> Self {
        if let Ok(mut attempts) = self.attempts.lock() {
            attempts.max = max;
        }
        self
    }

    pub fn lockout(self, minutes: i64) -> Self {
        if let Ok(mut attempts) = self.attempts.lock() {
            attempts.minutes = minutes;
        }
        self
    }

    pub fn session(&self, routes: Routes) -> Routes {
        routes.layer(from_fn_with_state(self.clone(), Self::load))
    }

    pub fn require_login(&self, routes: Routes) -> Routes {
        routes.layer(from_fn_with_state(self.clone(), Self::deny))
    }

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
        let raw = cookie(req.headers(), &self.cookie).and_then(|raw| claim(&raw));
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
        Repository::<User>::new(store)
            .get(&Value::int(id))
            .await
            .ok()?
    }
}

#[derive(Clone)]
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
