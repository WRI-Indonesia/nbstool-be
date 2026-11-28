//! Everything no wired route matched (`*`): werkzeug's 404 page for a path Flask has no rule for,
//! its 405 page for a method no rule on that path has, flask-cors' OPTIONS answer, and the 308
//! redirect `merge_slashes` gives a path with `//` that matches a rule once merged.

use axum::response::Response;

use crate::core::{self, Req};

/// Every live rule of Flask's url_map (blueprint prefix + rule; `# off:` routes left out, no
/// static folder exists) with its methods. `<x>` is werkzeug's default converter: one non-empty
/// segment. Flask adds HEAD to GET rules and OPTIONS to every rule.
const FLASK_RULES: &[(&str, &[&str])] = &[
    ("/", &["GET"]),
    ("/api/v1/docs", &["GET"]),
    ("/api/v1/docs/specs", &["GET"]),
    ("/documents/ccb", &["GET", "POST"]),
    ("/documents/cleanup", &["GET"]),
    ("/documents/cleanup/reminder", &["GET"]),
    ("/documents/csv", &["GET"]),
    ("/documents/docx", &["GET", "DELETE"]),
    ("/documents/download/csv", &["GET"]),
    ("/documents/download/docx", &["GET"]),
    ("/documents/download/xlsx", &["GET"]),
    // ADR-013: the legacy v2 `POST /documents/feasibility` is dropped, so it has no rule here
    ("/documents/feasibility/v3", &["GET", "POST"]),
    ("/documents/geopdf", &["GET"]),
    ("/documents/kml", &["GET"]),
    ("/documents/monitoring/v3", &["GET", "POST"]),
    ("/documents/monitoring/v3/activities", &["GET"]),
    ("/geos/feature/analysis", &["GET"]),
    ("/geos/feature/benefit", &["POST"]),
    ("/geos/feature/benefit/people", &["POST"]),
    ("/geos/feature/pathway", &["GET"]),
    ("/geos/feature/site-characterisation", &["GET"]),
    ("/geos/feature/threat", &["GET"]),
    ("/geos/layers", &["GET"]),
    ("/geos/layers/v2", &["GET"]),
    ("/geos/map/explorer", &["GET", "POST"]),
    ("/geos/map/location", &["GET", "POST"]),
    ("/geos/map/location-search", &["GET", "POST"]),
    ("/geos/map/upload", &["POST"]),
    ("/geos/polygon", &["GET", "POST"]),
    ("/incubators/review", &["POST"]),
    ("/loggers/list", &["GET"]),
    ("/loggers/report-problem", &["POST"]),
    ("/loggers/user/activity", &["GET"]),
    ("/masters/dictionary/list", &["GET"]),
    ("/masters/faq/list", &["GET"]),
    ("/masters/organization", &["POST", "PUT"]),
    ("/masters/organization/list", &["GET"]),
    ("/projects", &["GET", "POST", "PUT", "DELETE"]),
    ("/projects/bind", &["POST"]),
    ("/projects/boundary-map", &["GET"]),
    ("/projects/documents", &["GET"]),
    ("/projects/list", &["GET"]),
    ("/projects/privacy", &["PATCH"]),
    ("/projects/statistics", &["GET"]),
    ("/users/account", &["GET", "POST"]),
    ("/users/account/change-password", &["POST"]),
    ("/users/account/forgot-password", &["POST"]),
    ("/users/account/get-token", &["GET"]),
    ("/users/account/login", &["POST"]),
    ("/users/account/register", &["POST"]),
    ("/users/account/request", &["POST"]),
    ("/users/account/reset-password", &["POST"]),
    ("/users/account/verification/<token>", &["GET"]),
    ("/users/account/verification/resend", &["POST"]),
    ("/users/ga4-report", &["GET"]),
];

use crate::core::NOT_FOUND_PAGE;

const METHOD_NOT_ALLOWED_PAGE: &str = "<!doctype html>\n<html lang=en>\n<title>405 Method Not Allowed</title>\n\
<h1>Method Not Allowed</h1>\n<p>The method is not allowed for the requested URL.</p>\n";

fn rule_matches(rule: &str, path: &str) -> bool {
    let (mut r, mut p) = (rule.split('/'), path.split('/'));
    loop {
        match (r.next(), p.next()) {
            (None, None) => return true,
            (Some(a), Some(b)) if a.starts_with('<') => {
                if b.is_empty() {
                    return false;
                }
            }
            (Some(a), Some(b)) if a == b => {}
            _ => return false,
        }
    }
}

/// werkzeug matches the percent-decoded path; a bad escape stays as sent.
fn decoded_path(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|b| char::from(*b).to_digit(16));
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The methods of every rule matching `path` (werkzeug's `have_match_for`); `None` when no rule
/// matches.
fn flask_methods(path: &str) -> Option<Vec<&'static str>> {
    let mut methods: Vec<&str> = Vec::new();
    let mut found = false;
    for (rule, rule_methods) in FLASK_RULES {
        if rule_matches(rule, path) {
            found = true;
            methods.extend_from_slice(rule_methods);
            if rule_methods.contains(&"GET") {
                methods.push("HEAD");
            }
            methods.push("OPTIONS");
        }
    }
    methods.sort_unstable_by_key(|m| (*m == "OPTIONS", *m));
    methods.dedup();
    found.then_some(methods)
}

/// werkzeug's `merge_slashes`: runs of `/` as one.
fn merge_slashes(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if !(c == '/' && out.ends_with('/')) {
            out.push(c);
        }
    }
    out
}

/// `RequestRedirect` (308) to the merged path: werkzeug's redirect page, the URL built from the
/// Host header (`http`, as the dev server and gunicorn without a proxy fix see it).
// shortcut: the path keeps the client's percent-escapes; werkzeug re-quotes the decoded path
// (the same text for every path made of URL-safe characters).
fn redirect(req: &Req) -> Response {
    let host = req
        .parts
        .headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let mut url = format!("http://{host}{}", merge_slashes(req.parts.uri.path()));
    if let Some(q) = req.parts.uri.query() {
        url.push('?');
        url.push_str(q);
    }
    let shown = crate::users_write::escape_html(&url);
    let page = format!(
        "<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n\
         <p>You should be redirected automatically to the target URL: <a href=\"{shown}\">{shown}</a>. \
         If not, click the link.\n"
    );
    core::with_header(
        core::respond(308, "text/html; charset=utf-8", page),
        "location",
        &url,
    )
}

pub async fn any(req: Req) -> Response {
    let path = decoded_path(req.parts.uri.path());
    let merged = path.contains("//");
    match flask_methods(&if merged { merge_slashes(&path) } else { path }) {
        None => core::respond(404, "text/html; charset=utf-8", NOT_FOUND_PAGE),
        Some(methods) if merged && methods.contains(&req.parts.method.as_str()) => redirect(&req),
        Some(methods) if req.parts.method == axum::http::Method::OPTIONS => options(&req, &methods),
        Some(methods) if methods.contains(&req.parts.method.as_str()) => {
            // A method Flask serves on this path that no route took. Flask would run that route;
            // these were sent to the original until the lift and are 404 since (the pure mode's
            // answer, kept on purpose; no recorded case reaches them): a percent-encoded path such
            // as `/projects%2Flist`, and GET/HEAD `/users/account/verification/resend` (Flask's
            // `<token>` rule; the static POST route shadows it here).
            core::respond(404, "text/html; charset=utf-8", NOT_FOUND_PAGE)
        }
        // werkzeug joins a set: its order varies per process; sorted here, OPTIONS last
        Some(methods) => core::with_header(
            core::respond(405, "text/html; charset=utf-8", METHOD_NOT_ALLOWED_PAGE),
            "allow",
            &methods.join(", "),
        ),
    }
}

/// `@cross_origin`'s OPTIONS answer: Flask's `make_default_options_response` (`Allow`: the
/// path's methods; a Python set, so its order varies per process) plus flask-cors' preflight
/// headers. The Origin echo and `Vary: Origin` are added for every answer (`core::cors`).
fn options(req: &Req, methods: &[&str]) -> Response {
    let mut response = core::respond(200, "text/html; charset=utf-8", "");
    response = core::with_header(response, "allow", &methods.join(", "));
    let header = |name: &str| {
        req.parts
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    if header("access-control-request-method").is_some() {
        response = core::with_header(
            response,
            "access-control-allow-methods",
            "DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT",
        );
        // get_allow_headers: the requested ones (allow_headers '*'), stripped and sorted
        if let Some(requested) = header("access-control-request-headers").filter(|h| !h.is_empty())
        {
            let mut names: Vec<&str> = requested.split(',').map(str::trim).collect();
            names.sort_unstable();
            response =
                core::with_header(response, "access-control-allow-headers", &names.join(", "));
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        let knows = |p: &str| flask_methods(p).is_some();
        assert!(knows("/"));
        assert!(knows("/projects"));
        assert!(knows("/users/account/verification/abc"));
        assert!(!knows("/users/account/verification/"));
        assert!(!knows("/projects/"));
        assert!(!knows("/nope"));
        assert!(!knows(""));
        assert!(knows(&decoded_path("/projects%2Flist")));
        assert_eq!(
            flask_methods("/projects/bind"),
            Some(vec!["POST", "OPTIONS"])
        );
        // the static rule and `<token>` both match
        assert_eq!(
            flask_methods("/users/account/verification/resend"),
            Some(vec!["GET", "HEAD", "POST", "OPTIONS"])
        );
    }
}
