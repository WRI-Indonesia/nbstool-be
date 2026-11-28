//! `application/apis/__init__.py`: the API reference page and its OpenAPI yaml.

use axum::http::header;
use axum::response::Response;

use crate::core::{self, Req};

const HTML: &str = "text/html; charset=utf-8";

/// `index.html` has no Jinja syntax, so `render_template` only does what Jinja's lexer does to
/// any template: newlines (`\r\n`, `\r`) become `\n` and one trailing newline is dropped
/// (`keep_trailing_newline=False`). The Windows checkout has CRLF, Linux LF; both render the same.
fn render_static(source: &str) -> String {
    let text = source.replace("\r\n", "\n").replace('\r', "\n");
    match text.strip_suffix('\n') {
        Some(t) => t.to_string(),
        None => text,
    }
}

/// `open(path, 'r').read()`: universal newlines; UTF-8 as on production Linux (the file is ASCII,
/// so Windows' cp1252 default would read it identically).
fn read_text(bytes: &[u8]) -> Result<String, ()> {
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| ())?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// `request.host`: the Host header (WSGI hands it over as latin-1), else the server address,
/// with werkzeug's `get_host` dropping the default `:80` of scheme http.
fn request_host(req: &Req) -> String {
    let host: String = match req.parts.headers.get(header::HOST) {
        Some(v) => v.as_bytes().iter().map(|&b| b as char).collect(),
        // shortcut: HTTP/2 has no Host header; the URI authority stands in for SERVER_NAME:PORT.
        None => req
            .parts
            .uri
            .authority()
            .map(|a| a.to_string())
            .unwrap_or_default(),
    };
    match host.strip_suffix(":80") {
        Some(h) => h.to_string(),
        None => host,
    }
}

/// An exception the view does not catch: no error handler is registered, so werkzeug's page.
fn unhandled() -> Response {
    core::respond(
        500,
        HTML,
        "<!doctype html>\n<html lang=en>\n<title>500 Internal Server Error</title>\n\
         <h1>Internal Server Error</h1>\n<p>The server encountered an internal error and was \
         unable to complete your request. Either the server is overloaded or there is an error \
         in the application.</p>\n",
    )
}

pub async fn landing(_req: Req) -> Response {
    // render_template exposes `session` to the template context, which adds `Vary: Cookie`
    core::vary_cookie(core::respond(
        200,
        HTML,
        render_static(include_str!(
            "../../../application/apis/_templates/index.html"
        )),
    ))
}

pub async fn specs(req: Req) -> Response {
    // embedded: it ships with the code it describes (the original opens it per request)
    match read_text(include_bytes!(
        "../../../application/apis/_templates/specs.yaml"
    )) {
        // request.scheme: gunicorn serves plain http and no ProxyFix is installed
        Ok(text) => core::respond(
            200,
            HTML,
            text.replace(
                "http://someserversurl.com",
                &format!("http://{}", request_host(&req)),
            ),
        ),
        Err(()) => unhandled(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(host: Option<&str>) -> Req {
        let mut b = axum::http::Request::builder().uri("/api/v1/docs/specs");
        if let Some(h) = host {
            b = b.header("host", h);
        }
        let (parts, _) = b.body(()).unwrap().into_parts();
        Req {
            parts,
            body: Default::default(),
        }
    }

    #[test]
    fn jinja_normalises_newlines_and_drops_last() {
        assert_eq!(render_static("a\r\nb\r\n"), "a\nb");
        assert_eq!(render_static("a\nb\n\n"), "a\nb\n");
        assert_eq!(render_static("a"), "a");
    }

    #[test]
    fn host_drops_default_port_only() {
        assert_eq!(request_host(&req(Some("example.com:80"))), "example.com");
        assert_eq!(request_host(&req(Some("127.0.0.1:5000"))), "127.0.0.1:5000");
        assert_eq!(request_host(&req(None)), "");
    }

    #[tokio::test]
    async fn specs_matches_recording() {
        let resp = specs(req(Some("127.0.0.1:5001"))).await;
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let recorded = include_str!("../../app-cases/http/docs-specs.body");
        let mut rest = std::str::from_utf8(&body).unwrap();
        // "[..]" is a wildcard: match the literal pieces in order
        for (i, piece) in recorded.split("[..]").enumerate() {
            if i == 0 {
                assert!(rest.starts_with(piece));
                rest = &rest[piece.len()..];
            } else {
                let at = rest.find(piece).expect("piece in order");
                rest = &rest[at + piece.len()..];
            }
        }
        assert!(rest.is_empty());
    }
}
