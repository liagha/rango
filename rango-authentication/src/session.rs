#[cfg(feature = "views")]
use axum::extract::Extension;
#[cfg(feature = "views")]
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use axum::extract::OriginalUri;
#[cfg(feature = "views")]
use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
#[cfg(feature = "views")]
use rango_core::forgery::Token;
pub(crate) use rango_core::forgery::named as cookie;
use rango_core::view::Request;
use sha2::Sha256;

pub(crate) type Claim = (i64, i64, String);

pub(crate) struct Attempts {
    pub(crate) max: u32,
    pub(crate) minutes: i64,
    #[cfg(feature = "views")]
    hits: HashMap<String, (u32, Instant)>,
}

impl Attempts {
    pub(crate) fn new(max: u32, minutes: i64) -> Self {
        Self {
            max,
            minutes,
            #[cfg(feature = "views")]
            hits: HashMap::new(),
        }
    }

    #[cfg(feature = "views")]
    pub(crate) fn blocked(&mut self, name: &str) -> bool {
        self.sweep();
        self.hits
            .get(name)
            .is_some_and(|(fails, _)| *fails >= self.max)
    }

    #[cfg(feature = "views")]
    pub(crate) fn fail(&mut self, name: &str) {
        let window = self.window();
        let now = Instant::now();
        let hit = self.hits.entry(name.into()).or_insert((0, now));
        if now.duration_since(hit.1) >= window {
            *hit = (1, now);
        } else {
            hit.0 += 1;
        }
    }

    #[cfg(feature = "views")]
    pub(crate) fn clear(&mut self, name: &str) {
        self.hits.remove(name);
    }

    #[cfg(feature = "views")]
    fn window(&self) -> Duration {
        Duration::from_secs(self.minutes.max(1) as u64 * 60)
    }

    #[cfg(feature = "views")]
    fn sweep(&mut self) {
        let window = self.window();
        let now = Instant::now();
        self.hits
            .retain(|_, (_, since)| now.duration_since(*since) < window);
    }
}

#[cfg(feature = "views")]
pub(crate) fn token(headers: &HeaderMap, guard: Option<Extension<Token>>) -> String {
    match guard {
        Some(Extension(token)) => token.0.clone(),
        None => cookie(headers, "forgery").unwrap_or_default(),
    }
}

#[cfg(feature = "views")]
pub(crate) fn sign(secret: &str, id: i64, exp: i64) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("auth secret takes any key");
    mac.update(format!("{id}.{exp}").as_bytes());
    hex_encode(&mac.finalize().into_bytes())
}

pub(crate) fn verify(secret: &str, id: i64, exp: i64, sig: &str) -> bool {
    let Some(want) = hex_decode(sig) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(format!("{id}.{exp}").as_bytes());
    mac.verify_slice(&want).is_ok()
}

pub(crate) fn claim(raw: &str) -> Option<Claim> {
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
pub(crate) fn safe_next(raw: Option<String>) -> Option<String> {
    raw.filter(|to| to.starts_with('/') && !to.starts_with("//"))
}

pub(crate) fn login_url(login: &str, req: &Request) -> String {
    let back = req
        .extensions()
        .get::<OriginalUri>()
        .map(|uri| {
            uri.0
                .path_and_query()
                .map(|part| part.as_str())
                .unwrap_or("/")
                .to_string()
        })
        .unwrap_or_else(|| {
            req.uri()
                .path_and_query()
                .map(|part| part.as_str())
                .unwrap_or("/")
                .to_string()
        });
    let next = serde_urlencoded::to_string([("next", back)]).unwrap_or_default();
    format!("{login}?{next}")
}

#[cfg(feature = "views")]
pub(crate) fn set_cookie(name: &str, raw: &str, age: i64) -> axum::http::HeaderValue {
    axum::http::HeaderValue::from_str(&format!(
        "{name}={raw}; Path=/; Max-Age={age}; HttpOnly; SameSite=Lax"
    ))
    .expect("session cookie is header-safe")
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::COOKIE;

    #[test]
    fn cookie_ok() {
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, "a=1; session=abc; b=2".parse().unwrap());
        assert_eq!(cookie(&headers, "session"), Some("abc".into()));
        assert_eq!(cookie(&headers, "missing"), None);
    }

    #[test]
    fn claim_ok() {
        assert_eq!(claim("7.999.ab12"), Some((7, 999, "ab12".into())));
        assert_eq!(claim("x"), None);
        assert_eq!(claim("1.2"), None);
        assert_eq!(claim("1.2.3.4"), None);
        assert_eq!(claim(""), None);
    }

    #[cfg(feature = "views")]
    #[test]
    fn codec() {
        let sig = sign("secret", 7, 999);
        assert!(verify("secret", 7, 999, &sig));
        assert!(!verify("other", 7, 999, &sig));
        assert!(!verify("secret", 8, 999, &sig));
        assert!(!verify("secret", 7, 999, "deadbeef"));
    }

    #[cfg(feature = "views")]
    #[test]
    fn next_filter() {
        assert_eq!(safe_next(Some("/admin/".into())), Some("/admin/".into()));
        assert_eq!(safe_next(Some("https://evil.example".into())), None);
        assert_eq!(safe_next(Some("//evil.example".into())), None);
        assert_eq!(safe_next(None), None);
    }

    #[cfg(feature = "views")]
    #[test]
    fn lockout() {
        let mut attempts = Attempts::new(3, 15);
        assert!(!attempts.blocked("u"));
        attempts.fail("u");
        attempts.fail("u");
        assert!(!attempts.blocked("u"));
        attempts.fail("u");
        assert!(attempts.blocked("u"));
        assert!(!attempts.blocked("other"));
        attempts.clear("u");
        assert!(!attempts.blocked("u"));
    }
}
