use std::sync::Arc;

use askama::Template;
use axum::{
    extract::{Extension, Form, Query},
    http::{HeaderMap, header::SET_COOKIE},
    routing::{get, post},
};
use rango_core::{
    Error, Repository, Store,
    chrono::Utc,
    forgery::Token,
    urls::Routes,
    view::{self, render},
};

use super::{Authentication, Current, User};
use crate::session::{safe_next, set_cookie, sign, token};

impl Authentication {
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
                    move |store: Extension<Arc<dyn Store>>,
                          headers: HeaderMap,
                          guard: Option<Extension<Token>>,
                          Form(form): Form<LoginForm>| {
                        let enter = enter.clone();
                        async move { enter.enter(store.0, headers, guard, form).await }
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

    async fn show(
        &self,
        current: Current,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
        next: Option<String>,
    ) -> Result<axum::response::Response, Error> {
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

    async fn enter(
        &self,
        store: Arc<dyn Store>,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
        form: LoginForm,
    ) -> Result<axum::response::Response, Error> {
        let next = safe_next(form.next.clone()).unwrap_or_else(|| "/".into());
        let denied = |error: &str| {
            render(Login {
                error: error.into(),
                username: form.username.clone(),
                user: None,
                token: token(&headers, guard.clone()),
                next: safe_next(form.next.clone()).unwrap_or_default(),
            })
        };
        let locked = self
            .attempts
            .lock()
            .map(|mut attempts| attempts.blocked(&form.username))
            .unwrap_or(true);
        if locked {
            return denied("Too many attempts. Try again later.");
        }
        match User::login(store, &form.username, &form.password).await? {
            Some(user) => {
                if let Ok(mut attempts) = self.attempts.lock() {
                    attempts.clear(&user.username);
                }
                let mut response = view::redirect(&next);
                response
                    .headers_mut()
                    .insert(SET_COOKIE, self.cookie_for(&user));
                Ok(response)
            }
            None => {
                if let Ok(mut attempts) = self.attempts.lock() {
                    attempts.fail(&form.username);
                }
                denied("Invalid username or password.")
            }
        }
    }

    fn cookie_for(&self, user: &User) -> axum::http::HeaderValue {
        let exp = Utc::now().timestamp() + self.days * 86400;
        let raw = format!("{}.{}.{}", user.id, exp, sign(&self.secret, user.id, exp));
        set_cookie(&self.cookie, &raw, self.days * 86400)
    }

    async fn show_signup(
        &self,
        current: Current,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
    ) -> Result<axum::response::Response, Error> {
        if current.0.is_some() {
            return Ok(view::redirect("/"));
        }
        render(Signup {
            error: String::new(),
            username: String::new(),
            token: token(&headers, guard),
        })
    }

    async fn create(
        &self,
        store: Arc<dyn Store>,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
        form: SignupForm,
    ) -> Result<axum::response::Response, Error> {
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
        match User::register(store, &form.username, &form.password, false).await {
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

    async fn show_password(
        &self,
        current: Current,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
    ) -> Result<axum::response::Response, Error> {
        if current.0.is_none() {
            return Ok(view::redirect(&self.login));
        }
        render(Password {
            error: String::new(),
            done: false,
            token: token(&headers, guard),
        })
    }

    async fn change(
        &self,
        store: Arc<dyn Store>,
        current: Current,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
        form: PasswordForm,
    ) -> Result<axum::response::Response, Error> {
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

    async fn exit(&self) -> axum::response::Response {
        let mut response = view::redirect(&self.login);
        response
            .headers_mut()
            .insert(SET_COOKIE, set_cookie(&self.cookie, "", 0));
        response
    }
}

#[derive(Template)]
#[template(path = "login.html")]
struct Login {
    error: String,
    username: String,
    user: Option<String>,
    token: String,
    next: String,
}

#[derive(rango_core::serde::Deserialize)]
#[serde(crate = "rango_core::serde")]
struct LoginForm {
    username: String,
    password: String,
    next: Option<String>,
}

#[derive(rango_core::serde::Deserialize)]
#[serde(crate = "rango_core::serde")]
struct NextQuery {
    next: Option<String>,
}

#[derive(Template)]
#[template(path = "register.html")]
struct Signup {
    error: String,
    username: String,
    token: String,
}

#[derive(rango_core::serde::Deserialize)]
#[serde(crate = "rango_core::serde")]
struct SignupForm {
    username: String,
    password: String,
    confirm: String,
}

#[derive(Template)]
#[template(path = "password.html")]
struct Password {
    error: String,
    done: bool,
    token: String,
}

#[derive(rango_core::serde::Deserialize)]
#[serde(crate = "rango_core::serde")]
struct PasswordForm {
    current: String,
    password: String,
    confirm: String,
}
