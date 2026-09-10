use std::sync::Arc;

use axum::{
    extract::Extension,
    routing::{get, post},
};
use rango::{
    model::{Model, Schema},
    urls::Routes,
};

mod form;
mod history;
mod query;
mod row;
mod views;

use history::History;
use views::{create, dashboard, detail, list, remove, replace, show_edit, show_new};

pub fn history() -> Schema {
    History::schema()
}

pub struct Admin {
    routes: Routes,
    models: Vec<Schema>,
}

impl Admin {
    pub fn new() -> Self {
        let routes = Routes::new().route("/", get(dashboard));
        Self {
            routes,
            models: Vec::new(),
        }
    }

    pub fn model<M: Model>(mut self) -> Self {
        self.models.push(M::schema());
        self.routes = self.routes.merge(model_routes::<M>(M::table()));
        self
    }

    pub fn routes(self) -> Routes {
        self.routes.layer(Extension(Arc::new(self.models)))
    }
}

impl Default for Admin {
    fn default() -> Self {
        Self::new()
    }
}

fn model_routes<M: Model>(table: &str) -> Routes {
    Routes::new()
        .route(format!("/{table}/"), get(list::<M>))
        .route(
            format!("/{table}/new/"),
            get(show_new::<M>).post(create::<M>),
        )
        .route(format!("/{table}/{{id}}/"), get(detail::<M>))
        .route(
            format!("/{table}/{{id}}/edit/"),
            get(show_edit::<M>).post(replace::<M>),
        )
        .route(format!("/{table}/{{id}}/delete/"), post(remove::<M>))
}
