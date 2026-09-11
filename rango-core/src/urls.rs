use std::convert::Infallible;

use axum::{
    Router,
    extract::Request,
    response::IntoResponse,
    routing::{MethodRouter, Route},
};
use tower::{Layer, Service};

pub struct Routes {
    router: Router,
}

impl Routes {
    pub fn new() -> Self {
        Self {
            router: Router::new(),
        }
    }

    pub fn route(mut self, path: impl Into<String>, method: MethodRouter) -> Self {
        let path = path.into();
        self.router = self.router.route(&path, method);
        self
    }

    pub fn merge(mut self, other: Routes) -> Self {
        self.router = self.router.merge(other.router);
        self
    }

    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        self.router = self.router.layer(layer);
        self
    }

    pub fn into_router(self) -> Router {
        self.router
    }
}

impl Default for Routes {
    fn default() -> Self {
        Self::new()
    }
}
