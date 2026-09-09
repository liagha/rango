use std::any::Any;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tower_http::catch_panic::CatchPanicLayer;

pub fn catch_panic(
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
