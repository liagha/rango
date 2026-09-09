use axum::{
    body::to_bytes,
    http::{
        Method,
        header::{COOKIE, HeaderMap, HeaderValue, SET_COOKIE},
    },
    middleware::Next,
};

use crate::{
    error::Error,
    view::{Request, Response},
};

pub const LIMIT: usize = 2 * 1024 * 1024;

const NAME: &str = "csrf";

#[derive(Clone)]
pub struct Token(pub String);

fn generate() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn safe(method: &Method) -> bool {
    matches!(
        method,
        &Method::GET | &Method::HEAD | &Method::OPTIONS | &Method::TRACE
    )
}

fn parse_field(body: &[u8]) -> Option<String> {
    let map: std::collections::HashMap<String, String> = serde_urlencoded::from_bytes(body).ok()?;
    map.get(NAME).cloned()
}

fn signed(token: &str) -> HeaderValue {
    HeaderValue::from_str(&format!("{NAME}={token}; Path=/; HttpOnly; SameSite=Lax")).unwrap()
}

pub fn cookie(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(COOKIE)?.to_str().ok()?;
    value.split(';').map(str::trim).find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == NAME).then(|| value.to_string())
    })
}

pub fn token(req: &Request) -> String {
    if let Some(token) = req.extensions().get::<Token>() {
        return token.0.clone();
    }
    cookie(req.headers()).unwrap_or_else(generate)
}

pub async fn guard(req: Request, next: Next) -> Result<Response, Error> {
    let safe = safe(req.method());
    let (mut parts, body) = req.into_parts();
    let bytes = to_bytes(body, LIMIT)
        .await
        .map_err(|_| Error::BadRequest("body too large".into()))?;
    let existing = cookie(&parts.headers);

    if !safe {
        let Some(token) = existing.clone() else {
            return Err(Error::Forbidden);
        };
        if parse_field(&bytes).as_deref() != Some(token.as_str()) {
            return Err(Error::Forbidden);
        }
    }

    let token = existing.clone().unwrap_or_else(generate);
    parts.extensions.insert(Token(token.clone()));
    let mut response = next.run(Request::from_parts(parts, bytes.into())).await;

    if existing.is_none() {
        response.headers_mut().insert(SET_COOKIE, signed(&token));
    }
    Ok(response)
}
