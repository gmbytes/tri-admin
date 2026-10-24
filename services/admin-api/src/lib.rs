//! Remote management API for tri-admin.

pub mod bootstrap;
pub mod common;
pub mod config;
pub mod modules;
pub mod state;
pub mod storage;

use axum::{Router, routing::get};
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
        ]);
    Router::new()
        .merge(modules::roles::router::routes())
        .merge(modules::users::router::routes())
        .merge(modules::menus::router::routes())
        .nest("/api/v1/auth", modules::auth::router::routes())
        .route("/health/live", get(modules::health::live))
        .route("/health/ready", get(modules::health::ready))
        .fallback(|| async { common::error::AppError::NotFound })
        .with_state(state)
        .layer(cors)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}
