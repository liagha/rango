//! Views: request handlers and response helpers.

use askama::Template;
use axum::{
    response::{Html, IntoResponse, Redirect},
    routing::{MethodRouter, get},
};
use serde::Serialize;

use crate::error::Error;

pub use axum::extract::Json;

/// Incoming axum request.
pub type Request = axum::extract::Request;
/// Outgoing axum response.
pub type Response = axum::response::Response;

/// HTML response with the given body.
pub fn html(body: impl Into<String>) -> Response {
    Html(body.into()).into_response()
}

/// JSON response from a serializable value.
pub fn json(data: impl Serialize) -> Response {
    Json(data).into_response()
}

/// HTML response from rendering an askama template.
pub fn render<T: Template>(template: T) -> Result<Response, Error> {
    Ok(Html(template.render()?).into_response())
}

/// Redirect response to the given location.
pub fn redirect(to: &str) -> Response {
    Redirect::to(to).into_response()
}

/// A request handler producing a response or an error.
pub trait View: Clone + Send + Sync + 'static {
    /// Run this view against the request.
    fn call(self, req: Request) -> Result<Response, Error>;
}

impl<F> View for F
where
    F: Fn(Request) -> Result<Response, Error> + Clone + Send + Sync + 'static,
{
    fn call(self, req: Request) -> Result<Response, Error> {
        self(req)
    }
}

/// Axum method router for a view closure.
pub fn get_view<V: View>(view: V) -> MethodRouter {
    get(move |req: Request| async move { view.call(req).into_response() })
}
