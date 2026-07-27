//! Static browser UI composition for native server hosts.

use std::path::PathBuf;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::any;
use tower_http::services::{ServeDir, ServeFile};

/// Attach a Vite-built SPA without allowing its fallback to mask bad API paths.
pub fn router(api: Router, frontend_dir: PathBuf) -> Router {
    let index = frontend_dir.join("index.html");
    let static_files = ServeDir::new(frontend_dir).fallback(ServeFile::new(index));
    api.route("/api/{*path}", any(api_not_found))
        .fallback_service(static_files)
}

async fn api_not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;

    fn fixture() -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("devicehub-spa-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("index.html"), "headless-ui").unwrap();
        std::fs::write(directory.join("asset.js"), "asset").unwrap();
        directory
    }

    #[tokio::test]
    async fn serves_assets_and_spa_routes_but_not_unknown_api_routes() {
        let directory = fixture();
        let app = router(
            Router::new().route("/api/status", get(|| async { "ok" })),
            directory.clone(),
        );

        for (path, status, body) in [
            ("/asset.js", StatusCode::OK, "asset"),
            ("/device", StatusCode::OK, "headless-ui"),
            ("/api/status", StatusCode::OK, "ok"),
            ("/api/missing", StatusCode::NOT_FOUND, ""),
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{path}");
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert_eq!(String::from_utf8_lossy(&bytes), body, "{path}");
        }

        std::fs::remove_dir_all(directory).unwrap();
    }
}
