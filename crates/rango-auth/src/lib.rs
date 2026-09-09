use std::sync::Arc;

#[cfg(feature = "views")]
use axum::{
    extract::{Extension, Form},
    http::header::SET_COOKIE,
    routing::{get, post},
};
use axum::{
    extract::{FromRequestParts, State},
    http::{HeaderMap, header::COOKIE, request::Parts},
    middleware::{Next, from_fn_with_state},
    response::Response,
};
use hmac::{Hmac, Mac};
use rango::{
    Error, Repo, Row, Store, StoreError, Value,
    model::{self, Field, Kind, Model},
    urls::Routes,
    view::{self, Request},
};
#[cfg(feature = "views")]
use rango::{csrf::Token, view::render};
use sha2::Sha256;

#[cfg(feature = "views")]
use askama::Template;

#[derive(Clone)]
pub struct Auth {
    secret: String,
    login: String,
    cookie: String,
    days: i64,
}

impl Auth {
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            login: "/login".into(),
            cookie: "session".into(),
            days: 14,
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
        let routes = Routes::new()
            .route(
                "/login",
                get(
                    move |current: Current, headers: HeaderMap, guard: Option<Extension<Token>>| {
                        let show = show.clone();
                        async move { show.show(current, headers, guard).await }
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
            );
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
            view::redirect(&auth.login)
        }
    }

    fn peek(&self, req: &Request) -> (Option<Claim>, Option<Arc<dyn Store>>) {
        let raw = cookie(req.headers(), &self.cookie).and_then(|raw| parse(&raw));
        let store = req.extensions().get::<Arc<dyn Store>>().cloned();
        (raw, store)
    }

    async fn who(&self, raw: Option<Claim>, store: Option<Arc<dyn Store>>) -> Option<User> {
        let (id, exp, sig) = raw?;
        let store = store?;
        if exp < model::now() {
            return None;
        }
        if !check(&self.secret, id, exp, &sig) {
            return None;
        }
        Repo::<User>::new(store).get(id).await.ok()?
    }

    #[cfg(feature = "views")]
    async fn show(
        &self,
        current: Current,
        headers: HeaderMap,
        guard: Option<Extension<Token>>,
    ) -> Result<Response, Error> {
        render(Login {
            error: String::new(),
            username: String::new(),
            user: current.0.map(|user| user.username),
            token: token(&headers, guard),
        })
    }

    #[cfg(feature = "views")]
    async fn enter(&self, store: Arc<dyn Store>, form: LoginForm) -> Result<Response, Error> {
        match User::login(store, &form.username, &form.password).await? {
            Some(user) => {
                let exp = model::now() + self.days * 86400;
                let raw = format!("{}.{}.{}", user.id, exp, sign(&self.secret, user.id, exp));
                let mut response = view::redirect("/");
                response
                    .headers_mut()
                    .insert(SET_COOKIE, baked(&self.cookie, &raw, exp - model::now()));
                Ok(response)
            }
            None => render(Login {
                error: "Invalid username or password.".into(),
                username: form.username,
                user: None,
                token: String::new(),
            }),
        }
    }

    #[cfg(feature = "views")]
    async fn exit(&self) -> Response {
        let mut response = view::redirect(&self.login);
        response
            .headers_mut()
            .insert(SET_COOKIE, cleared(&self.cookie));
        response
    }
}

#[derive(Clone)]
pub struct Current(pub Option<User>);

type Claim = (i64, i64, String);
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

#[derive(Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password: String,
    pub created: i64,
}

impl Model for User {
    fn table() -> &'static str {
        "users"
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::id(),
            Field::new("username", Kind::Str).unique(),
            Field::new("password", Kind::Str),
            Field::new("created", Kind::DateTime),
        ]
    }

    fn row(&self) -> Vec<Value> {
        vec![
            Value::str(&self.username),
            Value::str(&self.password),
            Value::int(self.created),
        ]
    }

    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.int(0)?,
            username: row.str(1)?,
            password: row.str(2)?,
            created: row.int(3)?,
        })
    }

    fn set_id(&mut self, id: i64) {
        self.id = id;
    }

    fn id(&self) -> i64 {
        self.id
    }
}

impl User {
    pub async fn register(
        store: Arc<dyn Store>,
        username: &str,
        password: &str,
    ) -> Result<User, Error> {
        let username = username.trim();
        if username.is_empty() {
            return Err(Error::BadRequest("username is required".into()));
        }
        if password.len() < 8 {
            return Err(Error::BadRequest(
                "password must be at least 8 characters".into(),
            ));
        }
        let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
            .map_err(|fail| Error::Server(fail.to_string()))?;
        let mut user = User {
            id: 0,
            username: username.into(),
            password: hash,
            created: model::now(),
        };
        match Repo::new(store).save(&mut user).await {
            Ok(()) => Ok(user),
            Err(fail) if fail.to_string().contains("UNIQUE") => {
                Err(Error::BadRequest("username is taken".into()))
            }
            Err(fail) => Err(Error::Server(fail.to_string())),
        }
    }

    pub async fn login(
        store: Arc<dyn Store>,
        username: &str,
        password: &str,
    ) -> Result<Option<User>, Error> {
        let users = Repo::<User>::new(store).all().await?;
        for user in users {
            if user.username == username
                && bcrypt::verify(password, &user.password).unwrap_or(false)
            {
                return Ok(Some(user));
            }
        }
        Ok(None)
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
}

#[cfg(feature = "views")]
#[derive(rango::serde::Deserialize)]
#[serde(crate = "rango::serde")]
struct LoginForm {
    username: String,
    password: String,
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.split(';').map(str::trim).find_map(|part| {
                let (key, value) = part.split_once('=')?;
                (key == name).then(|| value.to_string())
            })
        })
}

#[cfg(feature = "views")]
fn token(headers: &HeaderMap, guard: Option<Extension<Token>>) -> String {
    match guard {
        Some(Extension(token)) => token.0.clone(),
        None => cookie(headers, "csrf").unwrap_or_default(),
    }
}

#[cfg(feature = "views")]
fn sign(secret: &str, id: i64, exp: i64) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("auth secret takes any key");
    mac.update(format!("{id}.{exp}").as_bytes());
    hex_encode(&mac.finalize().into_bytes())
}

fn check(secret: &str, id: i64, exp: i64, sig: &str) -> bool {
    let Some(want) = hex_decode(sig) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(format!("{id}.{exp}").as_bytes());
    mac.verify_slice(&want).is_ok()
}

fn parse(raw: &str) -> Option<Claim> {
    let mut parts = raw.split('.');
    let id = parts.next()?.parse::<i64>().ok()?;
    let exp = parts.next()?.parse::<i64>().ok()?;
    let sig = parts.next()?.to_string();
    if parts.next().is_some() {
        return None;
    }
    Some((id, exp, sig))
}

#[cfg(feature = "views")]
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 15) as usize] as char);
    }
    out
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn hex_decode(raw: &str) -> Option<Vec<u8>> {
    let bytes = raw.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        out.push(hex_val(pair[0])? << 4 | hex_val(pair[1])?);
    }
    Some(out)
}

#[cfg(feature = "views")]
fn baked(name: &str, raw: &str, age: i64) -> axum::http::HeaderValue {
    axum::http::HeaderValue::from_str(&format!(
        "{name}={raw}; Path=/; Max-Age={age}; HttpOnly; SameSite=Lax"
    ))
    .expect("session cookie is header-safe")
}

#[cfg(feature = "views")]
fn cleared(name: &str) -> axum::http::HeaderValue {
    axum::http::HeaderValue::from_str(&format!(
        "{name}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax"
    ))
    .expect("session cookie is header-safe")
}
