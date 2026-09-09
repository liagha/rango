use std::{any::Any, sync::Arc};

use axum::{Router, extract::Extension};
#[cfg(feature = "forgery")]
use axum::{extract::DefaultBodyLimit, middleware::from_fn};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use tower_http::{catch_panic::CatchPanicLayer, services::ServeDir, trace::TraceLayer};

#[cfg(feature = "forgery")]
use crate::forgery;
use crate::{error::Error, settings::Settings, store::Store, urls::Routes};

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
        let dir = self.settings.assets.clone();
        self.router = self.router.nest_service("/assets", ServeDir::new(dir));
        self
    }

    fn build(self) -> Router {
        let mut router = self
            .router
            .fallback(|| async { Error::NotFound.into_response() });
        router = router.layer(Extension(self.store));
        #[cfg(feature = "forgery")]
        if self.settings.forgery {
            router = router.layer(from_fn(forgery::guard));
        }
        router = router
            .layer(TraceLayer::new_for_http())
            .layer(catch_panic(self.settings.debug));
        #[cfg(feature = "forgery")]
        if self.settings.forgery {
            router = router.layer(DefaultBodyLimit::max(forgery::LIMIT));
        }
        router
    }

    pub async fn run(self) -> Result<(), std::io::Error> {
        if self.settings.secret.is_none() {
            return Err(std::io::Error::other("set a secret before serving"));
        }
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

fn catch_panic(
    debug: bool,
) -> CatchPanicLayer<impl FnMut(Box<dyn Any + Send + 'static>) -> Response + Clone> {
    CatchPanicLayer::custom(move |panic: Box<dyn Any + Send + 'static>| {
        let body = if debug {
            panic
                .downcast_ref::<&str>()
                .map(|msg| msg.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "Internal Server Error".to_string())
        } else {
            "Internal Server Error".to_string()
        };
        (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
    })
}
