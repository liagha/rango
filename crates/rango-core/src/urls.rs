use axum::{Router, routing::MethodRouter};

pub struct Routes {
    routes: Vec<(String, MethodRouter)>,
}

impl Routes {
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    pub fn route(mut self, path: impl Into<String>, method: MethodRouter) -> Self {
        self.routes.push((path.into(), method));
        self
    }

    pub fn merge(mut self, other: Routes) -> Self {
        self.routes.extend(other.routes);
        self
    }

    pub fn into_router(self) -> Router {
        let mut router = Router::new();
        for (path, method) in self.routes {
            router = router.route(&path, method);
        }
        router
    }
}

impl Default for Routes {
    fn default() -> Self {
        Self::new()
    }
}
