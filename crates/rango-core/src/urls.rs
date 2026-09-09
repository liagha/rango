use axum::{Router, routing::MethodRouter};

pub struct Routes {
    router: Router,
}

impl Routes {
    pub fn new() -> Self {
        Self {
            router: Router::new(),
        }
    }

    pub fn route(mut self, path: &'static str, method: MethodRouter) -> Self {
        self.router = self.router.route(path, method);
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
