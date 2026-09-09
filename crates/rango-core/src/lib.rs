pub mod app;
#[cfg(feature = "csrf")]
pub mod csrf;
pub mod error;
pub mod form;
pub mod middleware;
pub mod model;
pub mod prelude;
pub mod repo;
pub mod settings;
pub mod urls;
pub mod view;

pub use rango_store as store;
pub use store::{Row, Store, StoreError, Value};

pub use app::App;
#[cfg(feature = "csrf")]
pub use csrf::{Token, cookie, guard, token};
pub use error::Error;
pub use form::{Errors, FieldError, Form, Valid};
pub use model::{Field, Kind, Model};
pub use repo::Repo;
pub use settings::Settings;
pub use urls::Routes;
pub use view::{Request, Response, View, get_view, html, not_found, redirect, render};

pub use askama;
pub use serde;
pub use tokio;

pub use askama::Template;
pub use axum::routing::{get, post};
