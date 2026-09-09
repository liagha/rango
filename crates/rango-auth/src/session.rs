#[cfg(feature = "views")]
use axum::extract::Extension;
use axum::{
    extract::OriginalUri,
    http::{HeaderMap, header::COOKIE},
};
use hmac::{Hmac, Mac};
#[cfg(feature = "views")]
use rango::forgery::Token;
use rango::view::Request;
use sha2::Sha256;

pub(crate) type Claim = (i64, i64, String);

pub(crate) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
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
