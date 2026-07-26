use axum::Router;
use tower_http::cors::CorsLayer;

pub use devicehub_server::private_api::PrivateApiState as AppState;

pub fn router(state: AppState, token: String) -> Router {
    devicehub_server::private_api::router(state, token).layer(CorsLayer::permissive())
}
