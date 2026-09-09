use std::sync::Arc;

#[cfg(feature = "views")]
use axum::extract::Query;
#[cfg(feature = "views")]
use axum::http::HeaderMap;
#[cfg(feature = "views")]
use axum::{
    extract::{Extension, Form},
    http::header::SET_COOKIE,
    routing::{get, post},
};
use axum::{
    extract::{FromRequestParts, State},
    http::request::Parts,
    middleware::{Next, from_fn_with_state},
    response::Response,
};
use rango::{
    Error, Repository, Store,
    chrono::Utc,
    urls::Routes,
    view::{self, Request},
};
#[cfg(feature = "views")]
use rango::{forgery::Token, view::render};

mod session;
mod user;

use session::{Claim, claim, cookie, login_url, verify};
#[cfg(feature = "views")]
use session::{safe_next, set_cookie, sign, token};
pub use user::User;

#[cfg(feature = "views")]
use askama::Template;

#[derive(Clone)]
pub struct Auth {
    secret: String,
    login: String,
    cookie: String,
    days: i64,
    signup: bool,
}

impl Auth {
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            login: "/login".into(),
            cookie: "session".into(),
            days: 14,
            signup: false,
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

    pub fn session(&self, routes: Routes) -> Routes {
        routes.layer(from_fn_with_state(self.clone(), Self::load))
    }

    pub fn require_login(&self, routes: Routes) -> Routes {
        routes.layer(from_fn_with_state(self.clone(), Self::deny))
    }

    #[cfg(feature = "views")]
    pub fn routes(&self) -> Routes {
        let show = self.clone();
        let enter = self.clone();
        let exit = self.clone();
        let signup_show = self.clone();
        let signup_enter = self.clone();
        let password_show = self.clone();
        let password_change = self.clone();
        let mut routes = Routes::new()
            .route(
                "/login",
                get(
                    move |current: Current,
                          headers: HeaderMap,
                          guard: Option<Extension<Token>>,
                          Query(query): Query<NextQuery>| {
                        let show = show.clone();
                        async move { show.show(current, headers, guard, query.next).await }
                    },
                )
                .post(
                    move |store: Extension<Arc<dyn Store>>, Form(form): Form<LoginForm>| {
                        let enter = enter.clone();
                        async move { enter.enter(store.0, form).await }
                    },
                ),
            )
            .route(
                "/logout",
                post(move || {
                    let exit = exit.clone();
                    async move { exit.exit().await }
                }),
            )
            .route(
                "/password",
                get(
                    move |current: Current, headers: HeaderMap, guard: Option<Extension<Token>>| {
                        let password_show = password_show.clone();
                        async move { password_show.show_password(current, headers, guard).await }
                    },
                )
                .post(
                    move |store: Extension<Arc<dyn Store>>,
                          current: Current,
                          headers: HeaderMap,
                          guard: Option<Extension<Token>>,
                          Form(form): Form<PasswordForm>| {
                        let password_change = password_change.clone();
                        async move {
                            password_change
                                .change(store.0, current, headers, guard, form)
                                .await
                        }
                    },
                ),
            );
        if self.signup {
            routes = routes.route(
                "/register",
                get(
                    move |current: Current, headers: HeaderMap, guard: Option<Extension<Token>>| {
                        let signup_show = signup_show.clone();
                        async move { signup_show.show_signup(current, headers, guard).await }
                    },
                )
                .post(
                    move |store: Extension<Arc<dyn Store>>,
                          headers: HeaderMap,
                          guard: Option<Extension<Token>>,
                          Form(form): Form<SignupForm>| {
                        let signup_enter = signup_enter.clone();
                        async move { signup_enter.create(store.0, headers, guard, form).await }
                    },
                ),
            );
        }
        self.session(routes)
    }

    async fn load(State(auth): State<Auth>, mut req: Request, next: Next) -> Response {
        let (raw, store) = auth.peek(&req);
        let user = auth.who(raw, store).await;
        req.extensions_mut().insert(Current(user));
        next.run(req).await
    }

    async fn deny(State(auth): State<Auth>, mut req: Request, next: Next) -> Response {
        if req.extensions().get::<Current>().is_none() {
            let (raw, store) = auth.peek(&req);
            let user = auth.who(raw, store).await;
            req.extensions_mut().insert(Current(user));
        }
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
        Repository::<User>::new(store).get(id).await.ok()?
    }

    #[cfg(feature = "views")]
    async fn show(
        &self,
        current: Current,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
        next: Option<String>,
    ) -> Result<Response, Error> {
        let next = safe_next(next).unwrap_or_default();
        if let Some(user) = current.0 {
            if !next.is_empty() {
                return Ok(view::redirect(&next));
            }
            return render(Login {
                error: String::new(),
                username: String::new(),
                user: Some(user.username),
                token: token(&headers, guard),
                next: String::new(),
            });
        }
        render(Login {
            error: String::new(),
            username: String::new(),
            user: None,
            token: token(&headers, guard),
            next,
        })
    }

    #[cfg(feature = "views")]
    async fn enter(&self, store: Arc<dyn Store>, form: LoginForm) -> Result<Response, Error> {
        let next = safe_next(form.next.clone()).unwrap_or_else(|| "/".into());
        match User::login(store, &form.username, &form.password).await? {
            Some(user) => {
                let mut response = view::redirect(&next);
                response
                    .headers_mut()
                    .insert(SET_COOKIE, self.cookie_for(&user));
                Ok(response)
            }
            None => render(Login {
                error: "Invalid username or password.".into(),
                username: form.username,
                user: None,
                token: String::new(),
                next: safe_next(form.next).unwrap_or_default(),
            }),
        }
    }

    #[cfg(feature = "views")]
    fn cookie_for(&self, user: &User) -> axum::http::HeaderValue {
        let exp = Utc::now().timestamp() + self.days * 86400;
        let raw = format!("{}.{}.{}", user.id, exp, sign(&self.secret, user.id, exp));
        set_cookie(&self.cookie, &raw, self.days * 86400)
    }

    #[cfg(feature = "views")]
    async fn show_signup(
        &self,
        current: Current,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
    ) -> Result<Response, Error> {
        if current.0.is_some() {
            return Ok(view::redirect("/"));
        }
        render(Signup {
            error: String::new(),
            username: String::new(),
            token: token(&headers, guard),
        })
    }

    #[cfg(feature = "views")]
    async fn create(
        &self,
        store: Arc<dyn Store>,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
        form: SignupForm,
    ) -> Result<Response, Error> {
        let failed = |error: String| {
            render(Signup {
                error,
                username: form.username.clone(),
                token: token(&headers, guard.clone()),
            })
        };
        if form.password != form.confirm {
            return failed("Passwords do not match.".into());
        }
        match User::register(store, &form.username, &form.password).await {
            Ok(user) => {
                let mut response = view::redirect("/");
                response
                    .headers_mut()
                    .insert(SET_COOKIE, self.cookie_for(&user));
                Ok(response)
            }
            Err(Error::BadRequest(msg)) => failed(msg),
            Err(fail) => Err(fail),
        }
    }

    #[cfg(feature = "views")]
    async fn show_password(
        &self,
        current: Current,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
    ) -> Result<Response, Error> {
        if current.0.is_none() {
            return Ok(view::redirect(&self.login));
        }
        render(Password {
            error: String::new(),
            done: false,
            token: token(&headers, guard),
        })
    }

    #[cfg(feature = "views")]
    async fn change(
        &self,
        store: Arc<dyn Store>,
        current: Current,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
        form: PasswordForm,
    ) -> Result<Response, Error> {
        let Some(mut user) = current.0 else {
            return Ok(view::redirect(&self.login));
        };
        let failed = |error: String| {
            render(Password {
                error,
                done: false,
                token: token(&headers, guard.clone()),
            })
        };
        if !bcrypt::verify(&form.current, &user.password).unwrap_or(false) {
            return failed("Current password is incorrect.".into());
        }
        if form.password.len() < 8 {
            return failed("Password must be at least 8 characters.".into());
        }
        if form.password != form.confirm {
            return failed("Passwords do not match.".into());
        }
        user.password = bcrypt::hash(&form.password, bcrypt::DEFAULT_COST)
            .map_err(|fail| Error::Server(fail.to_string()))?;
        Repository::new(store).update(&user).await?;
        render(Password {
            error: String::new(),
            done: true,
            token: token(&headers, guard),
        })
    }

    #[cfg(feature = "views")]
    async fn exit(&self) -> Response {
        let mut response = view::redirect(&self.login);
        response
            .headers_mut()
            .insert(SET_COOKIE, set_cookie(&self.cookie, "", 0));
        response
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

#[cfg(feature = "views")]
#[derive(Template)]
#[template(path = "login.html")]
struct Login {
    error: String,
    username: String,
    user: Option<String>,
    token: String,
    next: String,
}

#[cfg(feature = "views")]
#[derive(rango::serde::Deserialize)]
#[serde(crate = "rango::serde")]
struct LoginForm {
    username: String,
    password: String,
    next: Option<String>,
}

#[cfg(feature = "views")]
#[derive(rango::serde::Deserialize)]
#[serde(crate = "rango::serde")]
struct NextQuery {
    next: Option<String>,
}

#[cfg(feature = "views")]
#[derive(Template)]
#[template(path = "register.html")]
struct Signup {
    error: String,
    username: String,
    token: String,
}

#[cfg(feature = "views")]
#[derive(rango::serde::Deserialize)]
#[serde(crate = "rango::serde")]
struct SignupForm {
    username: String,
    password: String,
    confirm: String,
}

#[cfg(feature = "views")]
#[derive(Template)]
#[template(path = "password.html")]
struct Password {
    error: String,
    done: bool,
    token: String,
}

#[cfg(feature = "views")]
#[derive(rango::serde::Deserialize)]
#[serde(crate = "rango::serde")]
struct PasswordForm {
    current: String,
    password: String,
    confirm: String,
}
