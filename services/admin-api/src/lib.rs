//! Remote management API for tri-admin.

pub mod bootstrap;
pub mod common;
pub mod config;
pub mod modules;
pub mod state;
pub mod storage;

use axum::Router;
use state::AppState;
use tower_http::{
    cors::CorsLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(state.config.cors_origins.clone())
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderName::from_static("x-request-id"),
        ])
        .expose_headers([
            axum::http::HeaderName::from_static("x-request-id"),
            axum::http::header::CONTENT_DISPOSITION,
            axum::http::header::ETAG,
        ]);
    let mut router = Router::new();
    for module in modules::registry() {
        let routes = (module.routes)();
        router = if module.prefix.is_empty() {
            router.merge(routes)
        } else {
            router.nest(module.prefix, routes)
        };
    }
    router
        .fallback(|| async { common::error::AppError::NotFound })
        .method_not_allowed_fallback(|| async { common::error::AppError::MethodNotAllowed })
        .with_state(state)
        .layer(cors)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}
