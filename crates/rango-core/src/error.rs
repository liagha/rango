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
    MethodNotAllowed,
    Server(String),
    Render(String),
}

impl Error {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
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
            Self::MethodNotAllowed => write!(f, "Method Not Allowed"),
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
