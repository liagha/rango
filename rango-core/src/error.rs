use std::fmt;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Debug)]
pub enum Error {
    BadRequest(String),
    Forbidden,
    NotFound,
    Server(String),
    Render(String),
}

impl Error {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Server(_) | Self::Render(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(msg) => write!(f, "{msg}"),
            Self::Forbidden => write!(f, "Forbidden"),
            Self::NotFound => write!(f, "Not Found"),
            Self::Server(msg) | Self::Render(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<askama::Error> for Error {
    fn from(value: askama::Error) -> Self {
        Self::Render(value.to_string())
    }
}

impl From<rango_store::StoreError> for Error {
    fn from(value: rango_store::StoreError) -> Self {
        Self::Server(value.to_string())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        if self.status().is_server_error() {
            tracing::error!(error = %self);
        }
        (self.status(), self.to_string()).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses() {
        assert_eq!(
            Error::BadRequest("x".into()).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(Error::Forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(Error::NotFound.status(), StatusCode::NOT_FOUND);
        assert!(Error::Server("x".into()).status().is_server_error());
        assert!(Error::Render("x".into()).status().is_server_error());
    }

    #[test]
    fn display() {
        assert_eq!(Error::BadRequest("msg".into()).to_string(), "msg");
        assert_eq!(Error::Forbidden.to_string(), "Forbidden");
        assert_eq!(Error::NotFound.to_string(), "Not Found");
        assert_eq!(Error::Server("msg".into()).to_string(), "msg");
    }
}
