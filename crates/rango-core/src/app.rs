use std::sync::Arc;

#[cfg(feature = "csrf")]
use axum::extract::DefaultBodyLimit;
#[cfg(feature = "csrf")]
use axum::middleware::from_fn;
use axum::{Router, extract::Extension};
use tower_http::{services::ServeDir, trace::TraceLayer};

#[cfg(feature = "csrf")]
use crate::csrf;
use crate::{middleware, settings::Settings, store::Store, urls::Routes, view};

pub struct App {
    settings: Settings,
    store: Arc<dyn Store>,
    router: Router,
}

impl App {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            store: crate::store::memory(),
            router: Router::new(),
        }
    }

    pub fn store(mut self, store: Arc<dyn Store>) -> Self {
        self.store = store;
        self
    }

    pub fn urls(mut self, routes: Routes) -> Self {
        self.router = self.router.merge(routes.into_router());
        self
    }

    pub fn mount(mut self, prefix: &str, routes: Routes) -> Self {
        self.router = self.router.nest(prefix, routes.into_router());
        self
    }

    pub fn mount_static(mut self) -> Self {
        let dir = self.settings.static_dir.clone();
        self.router = self.router.nest_service("/static", ServeDir::new(dir));
        self
    }

    fn build(self) -> Router {
        let mut router = self.router.fallback(|| async { view::not_found() });
        router = router.layer(Extension(self.store));
        #[cfg(feature = "csrf")]
        if self.settings.csrf {
            router = router.layer(from_fn(csrf::guard));
        }
        router = router
            .layer(TraceLayer::new_for_http())
            .layer(middleware::catch_panic(self.settings.debug));
        #[cfg(feature = "csrf")]
        if self.settings.csrf {
            router = router.layer(DefaultBodyLimit::max(csrf::LIMIT));
        }
        router
    }

    pub async fn run(self) -> Result<(), std::io::Error> {
        let address = (self.settings.host.as_str(), self.settings.port);
        let listener = tokio::net::TcpListener::bind(address).await?;
        tracing::info!("rango running on http://{}", self.settings.port);
        axum::serve(listener, self.build()).await
    }

    pub fn serve(self) -> Result<(), std::io::Error> {
        let _ = tracing_subscriber::fmt().try_init();
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(self.run())
    }
}
