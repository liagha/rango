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

const NAME: &str = "forgery";

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
    named(headers, NAME)
}

pub fn named(headers: &HeaderMap, name: &str) -> Option<String> {
    let value = headers.get(COOKIE)?.to_str().ok()?;
    value.split(';').map(str::trim).find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then(|| value.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::StatusCode,
        middleware,
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    #[test]
    fn cookies() {
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, "a=1; forgery=xyz; b=2".parse().unwrap());
        assert_eq!(cookie(&headers), Some("xyz".into()));
        assert_eq!(named(&headers, "b"), Some("2".into()));
        assert_eq!(named(&headers, "none"), None);
    }

    #[test]
    fn tokens() {
        let mut with_ext = axum::http::Request::<()>::builder().body(Body::empty()).unwrap();
        with_ext.extensions_mut().insert(Token("abc".into()));
        assert_eq!(token(&with_ext), "abc");
        let req = axum::http::Request::<()>::builder().body(Body::empty()).unwrap();
        assert_ne!(token(&req), token(&req));
    }

    async fn probe(req: Request) -> String {
        req.extensions()
            .get::<Token>()
            .map(|t| t.0.clone())
            .unwrap_or_default()
    }

    fn build(method: &str, set: Option<&str>, body: &'static str) -> axum::extract::Request {
        let mut request = axum::http::Request::<()>::builder()
            .method(method)
            .uri("/")
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(cookie) = set {
            request = request.header(COOKIE, cookie);
        }
        request.body(Body::from(body)).unwrap()
    }

    async fn body_of(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn app() -> Router {
        Router::new().route("/", get(probe)).layer(middleware::from_fn(guard))
    }

    #[tokio::test]
    async fn issues_token() {
        let res = app().oneshot(build("GET", None, "")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let issued = res
            .headers()
            .get(SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .strip_prefix("forgery=")
            .unwrap()
            .to_string();
        assert_eq!(issued, body_of(res).await);
    }

    #[tokio::test]
    async fn keeps_cookie() {
        let res = app()
            .oneshot(build("GET", Some("forgery=stated"), ""))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().get(SET_COOKIE).is_none());
        assert_eq!(body_of(res).await, "stated");
    }

    #[tokio::test]
    async fn guards_post() {
        let app = Router::new()
            .route("/", get(probe).post(probe))
            .layer(middleware::from_fn(guard));
        let denied = app
            .clone()
            .oneshot(build("POST", None, "forgery=nope"))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let wrong = app
            .clone()
            .oneshot(build("POST", Some("forgery=said"), "forgery=other"))
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
        let ok = app
            .oneshot(build("POST", Some("forgery=said"), "forgery=said"))
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(body_of(ok).await, "said");
    }

    #[tokio::test]
    async fn too_large() {
        let mut req = build("POST", Some("forgery=x"), "forgery=x");
        *req.body_mut() = Body::from(vec![b'x'; LIMIT + 1]);
        let res = app().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }
}
