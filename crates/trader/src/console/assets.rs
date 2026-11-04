//! The console's built files, embedded in the binary at build time (`build.rs`), served at `/`.
//! A path that names no file and no file type is a page of the app, which `index.html` routes.
//! A binary built without the console's files serves a page saying so.

use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

use crate::console::guard::refusal;

include!(concat!(env!("OUT_DIR"), "/console_assets.rs"));

/// The page a binary without the console's files serves.
const NOT_BUILT: &str = "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>trader console</title></head>\
<body><h1>The console is not built into this binary</h1><p>The API is served. To serve the console as well, build it \
(<code>npm ci</code> and <code>npm run build</code> in <code>console/</code>), then build the bot again.</p></body></html>\n";
/// Content types by file extension.
const TYPES: [(&str, &str); 11] = [
    ("html", "text/html; charset=utf-8"),
    ("js", "text/javascript; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("json", "application/json"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("ico", "image/x-icon"),
    ("woff2", "font/woff2"),
    ("woff", "font/woff"),
    ("txt", "text/plain; charset=utf-8"),
    ("map", "application/json"),
];

/// Serves the built file `uri` names, or the app's page.
pub async fn serve(method: Method, uri: Uri) -> Response {
    serve_from(ASSETS, &method, uri.path())
}

/// Serves `path` from `assets`: the file it names, `index.html` for `/` and for a path with no file
/// type, which is a page of the app, or the page saying the console is not built when there is no
/// `index.html`; and 404 for any other path, and for any under `/api/`, which no route took. Only
/// `GET` and `HEAD` are answered.
fn serve_from(assets: &'static [(&'static str, &'static [u8])], method: &Method, path: &str) -> Response {
    if path == "/api" || path.starts_with("/api/") {
        return refusal(StatusCode::NOT_FOUND, "no such route");
    }
    if !matches!(*method, Method::GET | Method::HEAD) {
        return refusal(StatusCode::METHOD_NOT_ALLOWED, "only GET and HEAD read the console's files");
    }
    let find = |wanted: &str| assets.iter().find(|(url, _)| *url == wanted);
    if let Some((url, bytes)) = find(path) {
        return file(url, bytes);
    }
    if path.rsplit('/').next().unwrap_or_default().contains('.') {
        return refusal(StatusCode::NOT_FOUND, "no such file");
    }
    match find("/index.html") {
        Some((url, bytes)) => file(url, bytes),
        None => file("/index.html", NOT_BUILT.as_bytes()),
    }
}

/// A file's answer: its content type, and a cache rule. Vite names each file under `/assets/` by a
/// hash of its content, so it never changes; any other file is checked on every use.
fn file(url: &str, bytes: &'static [u8]) -> Response {
    let extension = url.rsplit_once('.').map_or("", |(_, extension)| extension);
    let content_type =
        TYPES.iter().find(|(known, _)| *known == extension).map_or("application/octet-stream", |(_, t)| t);
    let cache = if url.starts_with("/assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    let headers =
        [(CONTENT_TYPE, HeaderValue::from_static(content_type)), (CACHE_CONTROL, HeaderValue::from_static(cache))];
    (headers, bytes).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    const BUILT: &[(&str, &[u8])] = &[
        ("/assets/app-1a2b.js", b"console.log(1)"),
        ("/favicon.svg", b"<svg/>"),
        ("/index.html", b"<html>app</html>"),
    ];

    /// Files are served by path with their type and cache rule; a page of the app gets the index.
    #[tokio::test]
    async fn serves_files_and_pages() {
        for (path, body, content_type, cache) in [
            ("/", "<html>app</html>", "text/html; charset=utf-8", "no-cache"),
            ("/accounts/0xabc", "<html>app</html>", "text/html; charset=utf-8", "no-cache"),
            (
                "/assets/app-1a2b.js",
                "console.log(1)",
                "text/javascript; charset=utf-8",
                "public, max-age=31536000, immutable",
            ),
            ("/favicon.svg", "<svg/>", "image/svg+xml", "no-cache"),
        ] {
            let response = serve_from(BUILT, &Method::GET, path);
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()[CONTENT_TYPE], content_type, "{path}");
            assert_eq!(response.headers()[CACHE_CONTROL], cache, "{path}");
            assert_eq!(to_bytes(response.into_body(), 1 << 20).await.unwrap(), body.as_bytes(), "{path}");
        }
    }

    /// A missing file, and a path under `/api/` no route took, are not answered with the app's
    /// page, which a browser would take for them.
    #[test]
    fn a_missing_file_is_not_found() {
        for assets in [BUILT, &[]] {
            assert_eq!(serve_from(assets, &Method::GET, "/assets/gone-9f9f.js").status(), StatusCode::NOT_FOUND);
            for path in ["/api", "/api/", "/api/accounts/0xabc/withdraw"] {
                for method in [Method::GET, Method::POST] {
                    assert_eq!(serve_from(assets, &method, path).status(), StatusCode::NOT_FOUND, "{method} {path}");
                }
            }
        }
    }

    /// Only reads are answered.
    #[test]
    fn only_reads() {
        assert_eq!(serve_from(BUILT, &Method::HEAD, "/").status(), StatusCode::OK);
        assert_eq!(serve_from(BUILT, &Method::POST, "/").status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    /// Without the console's files, every page says the console is not built.
    #[tokio::test]
    async fn without_a_build_says_so() {
        for path in ["/", "/accounts/0xabc"] {
            let response = serve_from(&[], &Method::GET, path);
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
            assert!(std::str::from_utf8(&body).unwrap().contains("The console is not built into this binary"));
        }
    }
}
