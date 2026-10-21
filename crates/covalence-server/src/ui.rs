//! Serves the built web UI (`packages/ui/dist`) next to the API.

use std::io;
use std::path::{Path, PathBuf};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

/// Where `pnpm --filter ui build` writes, relative to the repository root.
pub const DEFAULT_UI_DIR: &str = "packages/ui/dist";

/// Adds the UI in `dir` to `api`. API routes win; any other path is a file
/// from `dir`, or `index.html` when it names no file (SPA fallback). Screens
/// live in the URL hash, so the fallback only matters for links and reloads
/// that land on some other path.
pub fn with_ui(api: Router, dir: &Path) -> io::Result<Router> {
    let index = dir.join("index.html");
    if !index.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{} has no index.html: build the UI with `pnpm --filter ui build`",
                dir.display()
            ),
        ));
    }
    let spa = get(spa_index).with_state(index);
    let files = ServeDir::new(dir)
        .append_index_html_on_directories(false)
        .fallback(spa.clone());
    Ok(api.route("/", spa).fallback_service(files))
}

/// `index.html`, revalidated on every load so a new build is picked up. A
/// missing asset (`/assets/old.js`) is a 404, not HTML the browser can't run.
async fn spa_index(State(index): State<PathBuf>, request: Request) -> Response {
    let last = request.uri().path().rsplit('/').next().unwrap_or("");
    if last.contains('.') && last != "index.html" {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(mut response) = ServeFile::new(index).oneshot(request).await;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Method;
    use http_body_util::BodyExt;

    const INDEX: &str = "<!doctype html><div id=app></div>";

    fn dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), INDEX).unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/app-1234.js"), "console.log(1)").unwrap();
        dir
    }

    fn app(dir: &Path) -> Router {
        let api = Router::new()
            .route("/state", get(|| async { "api state" }))
            .route("/journal/{seq}/why", get(|| async { "api why" }));
        with_ui(api, dir).unwrap()
    }

    async fn call(app: &Router, method: Method, uri: &str) -> (StatusCode, Option<String>, String) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let cache = response
            .headers()
            .get(CACHE_CONTROL)
            .map(|v| v.to_str().unwrap().to_owned());
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, cache, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn serves_index_at_the_root_uncached() {
        let dir = dist();
        let (status, cache, body) = call(&app(dir.path()), Method::GET, "/").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(cache.as_deref(), Some("no-cache"));
        assert_eq!(body, INDEX);
    }

    #[tokio::test]
    async fn serves_assets() {
        let dir = dist();
        let (status, _, body) = call(&app(dir.path()), Method::GET, "/assets/app-1234.js").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "console.log(1)");
    }

    #[tokio::test]
    async fn api_routes_win() {
        let dir = dist();
        let app = app(dir.path());
        assert_eq!(call(&app, Method::GET, "/state").await.2, "api state");
        assert_eq!(call(&app, Method::GET, "/journal/3/why").await.2, "api why");
    }

    #[tokio::test]
    async fn falls_back_to_index_for_other_paths() {
        let dir = dist();
        let app = app(dir.path());
        for uri in ["/connections/new", "/some/deep/link", "/index.html"] {
            let (status, _, body) = call(&app, Method::GET, uri).await;
            assert_eq!((status, body.as_str()), (StatusCode::OK, INDEX), "{uri}");
        }
    }

    #[tokio::test]
    async fn a_missing_file_is_a_404() {
        let dir = dist();
        let (status, _, _) = call(&app(dir.path()), Method::GET, "/assets/old-5678.js").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn needs_a_built_ui() {
        let dir = tempfile::tempdir().unwrap();
        let err = with_ui(Router::new(), dir.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
