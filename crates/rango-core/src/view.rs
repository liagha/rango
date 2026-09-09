use askama::Template;
use axum::{
    response::{Html, IntoResponse, Redirect},
    routing::{MethodRouter, get},
};

use crate::error::Error;

pub type Request = axum::extract::Request;
pub type Response = axum::response::Response;

pub fn html(body: impl Into<String>) -> Response {
    Html(body.into()).into_response()
}

pub fn render<T: Template>(template: T) -> Result<Response, Error> {
    Ok(Html(template.render()?).into_response())
}

pub fn redirect(to: &str) -> Response {
    Redirect::to(to).into_response()
}

pub trait View: Clone + Send + Sync + 'static {
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

pub fn get_view<V: View>(view: V) -> MethodRouter {
    get(move |req: Request| async move { view.call(req).into_response() })
}
