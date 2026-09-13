//! Auto-generated admin panel for Rango: per-model search, edit, and row actions.

#![warn(missing_docs)]

use std::sync::Arc;

use axum::{
    extract::Extension,
    routing::{get, post},
};
use rango_core::{
    model::{Model, Schema, Table},
    urls::Routes,
};

mod form;
mod history;
mod query;
mod row;
mod views;

use history::History;
use views::{act, create, dashboard, detail, list, remove, replace, show_edit, show_new};

/// History ledger schema, exposed as an admin table of its own.
pub fn history() -> Schema {
    History::schema()
}

/// Builder for an admin panel over a set of registered models.
pub struct Admin {
    routes: Routes,
    models: Vec<Schema>,
}

impl Admin {
    /// Starts a builder with no registered models, serving `/` as the dashboard.
    pub fn new() -> Self {
        let routes = Routes::new().route("/", get(dashboard));
        Self {
            routes,
            models: Vec::new(),
        }
    }

    /// Registers a model and mounts its list, edit, and action routes.
    pub fn model<M: Model>(mut self) -> Self {
        self.models.push(M::schema());
        self.routes = self.routes.merge(model_routes::<M>(M::table()));
        self
    }

    /// Schemas of all registered models.
    pub fn schemas(&self) -> &[Schema] {
        &self.models
    }

    /// Finished route tree, layered with the registered models.
    pub fn routes(self) -> Routes {
        self.routes.layer(Extension(Arc::new(self.models)))
    }
}

impl Default for Admin {
    fn default() -> Self {
        Self::new()
    }
}

fn model_routes<M: Model>(table: Table) -> Routes {
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
        .route(format!("/{table}/actions/"), post(act::<M>))
}
