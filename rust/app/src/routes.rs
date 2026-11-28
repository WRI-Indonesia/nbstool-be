//! The route table, owned by hand. Each entry is `"METHOD /path"` (axum path syntax) and is
//! answered by `nbstool_api::handle` under that same name. A method a listed path does not have,
//! and every path not listed, goes to `*` (`api/src/not_found.rs`: werkzeug's 404 and 405 pages,
//! the OPTIONS answer, `//` redirects). MAP.md "Final route list" must list the same routes
//! (`tests/routes.rs`).

use std::collections::BTreeMap;

use axum::Router;
use axum::extract::Request;
use axum::response::Response;
use axum::routing::{MethodFilter, MethodRouter};

pub const ROUTES: &[&str] = &[
    "GET /",
    "GET /api/v1/docs",
    "GET /api/v1/docs/specs",
    "GET /documents/kml",
    "GET /documents/csv",
    "GET /masters/organization/list",
    "GET /masters/dictionary/list",
    "GET /masters/faq/list",
    "GET /geos/polygon",
    "GET /geos/map/explorer",
    "GET /geos/map/location",
    "GET /geos/map/location-search",
    "GET /geos/layers",
    "GET /geos/layers/v2",
    "GET /projects/statistics",
    "GET /projects/list",
    "GET /projects",
    "GET /projects/documents",
    "GET /users/account",
    "GET /users/account/get-token",
    "GET /loggers/user/activity",
    "GET /documents/monitoring/v3/activities",
    "GET /documents/feasibility/v3",
    "GET /documents/monitoring/v3",
    "POST /masters/organization",
    "PUT /masters/organization",
    "POST /projects",
    "PUT /projects",
    "PATCH /projects/privacy",
    "DELETE /projects",
    "POST /geos/polygon",
    "POST /geos/map/explorer",
    "POST /geos/map/location",
    "POST /geos/map/location-search",
    "POST /users/account",
    "POST /users/account/register",
    "POST /users/account/verification/resend",
    "GET /users/account/verification/{token}",
    "POST /users/account/forgot-password",
    "POST /users/account/reset-password",
    "POST /projects/bind",
    "POST /users/account/login",
    "POST /users/account/change-password",
    "POST /users/account/request",
    "GET /loggers/list",
    "GET /documents/docx",
    "DELETE /documents/docx",
    "GET /documents/ccb",
    "POST /documents/ccb",
    "POST /documents/feasibility/v3",
    "POST /documents/monitoring/v3",
    "POST /loggers/report-problem",
    "POST /incubators/review",
    "GET /users/ga4-report",
    "GET /documents/cleanup",
    "GET /documents/cleanup/reminder",
    "POST /geos/map/upload",
    "GET /geos/feature/threat",
    "POST /geos/feature/benefit",
    "POST /geos/feature/benefit/people",
    "GET /projects/boundary-map",
    "GET /documents/download/docx",
    "GET /documents/download/xlsx",
    "GET /documents/download/csv",
    "GET /documents/geopdf",
    "GET /geos/feature/site-characterisation",
    "GET /geos/feature/analysis",
    "GET /geos/feature/pathway",
];

/// The server's router: every entry of `ROUTES`, then `*` for the rest. A GET route also answers
/// HEAD (axum strips the body), as Flask does.
pub fn router() -> Router {
    let mut paths: BTreeMap<&str, MethodRouter> = BTreeMap::new();
    for &route in ROUTES {
        // INFALLIBLE: ROUTES is the constant above; `tests/routes.rs` checks every entry's shape.
        let (method, path) = route.split_once(' ').expect("`METHOD /path`");
        let filter = match method {
            "GET" => MethodFilter::GET,
            "POST" => MethodFilter::POST,
            "PUT" => MethodFilter::PUT,
            "PATCH" => MethodFilter::PATCH,
            "DELETE" => MethodFilter::DELETE,
            // INFALLIBLE: as above, ROUTES uses only these five methods.
            other => unreachable!("method {other} in ROUTES"),
        };
        let methods = paths.remove(path).unwrap_or_default();
        paths.insert(
            path,
            methods.on(filter, move |req: Request| nbstool_api::handle(route, req)),
        );
    }
    paths
        .into_iter()
        .fold(Router::new(), |router, (path, methods)| {
            router.route(path, methods.fallback(any))
        })
        .fallback(any)
}

/// `*`: a method the path does not have, or a path no route has.
async fn any(req: Request) -> Response {
    nbstool_api::handle("*", req).await
}
