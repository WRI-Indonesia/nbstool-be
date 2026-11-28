//! nbstool-be routes in Rust. `rust/app` calls `handle` for every route of its table
//! (`rust/app/src/routes.rs`) and with `*` for everything else.

pub mod core;
mod deflate;
mod docs;
mod documents_csv;
pub mod documents_generate;
mod documents_kml;
mod documents_read;
mod documents_write;
mod feature;
pub mod figures;
mod gcs;
mod gdrive;
mod geo_read;
mod geo_write;
mod layers;
pub mod logging;
mod masters;
mod masters_write;
mod not_found;
mod ops;
mod pickle;
mod pq;
mod projects;
mod projects_write;
mod pyzip;
mod sa;
mod sa_codes;
mod users;
mod users_write;

use axum::extract::Request;
use axum::response::Response;

pub async fn handle(route: &str, mut req: Request) -> Response {
    let loaded = core::UserLoaded::default();
    req.extensions_mut().insert(loaded.clone());
    let origin = req.headers().get(axum::http::header::ORIGIN).cloned();
    let req = match core::Req::read(req).await {
        Ok(req) => req,
        Err(response) => return response,
    };
    let response = match route {
        "GET /" => core::respond(200, "text/html; charset=utf-8", "ok"),
        "GET /api/v1/docs" => docs::landing(req).await,
        "GET /api/v1/docs/specs" => docs::specs(req).await,
        "GET /documents/kml" => documents_kml::kml(req).await,
        "GET /documents/csv" => documents_csv::csv(req).await,
        "GET /masters/organization/list" => masters::organization_list(req).await,
        "GET /masters/dictionary/list" => masters::dictionary_list(req).await,
        "GET /masters/faq/list" => masters::faq_list(req).await,
        "GET /geos/polygon" => geo_read::polygon_get(req).await,
        "GET /geos/map/explorer" => geo_read::map_explorer_get(req).await,
        "GET /geos/map/location" => geo_read::map_location_get(req).await,
        "GET /geos/map/location-search" => geo_read::map_location_search_get(req).await,
        "GET /geos/layers" => layers::layers_v1(req).await,
        "GET /geos/layers/v2" => layers::layers_v2(req).await,
        "GET /projects/statistics" => projects::statistics(req).await,
        "GET /projects/list" => projects::list(req).await,
        "GET /projects" => projects::detail(req).await,
        "GET /projects/documents" => projects::documents(req).await,
        "GET /projects/boundary-map" => projects::boundary_map(req).await,
        "GET /users/account" => users::account_get(req).await,
        "GET /users/account/get-token" => users::get_token(req).await,
        "GET /loggers/user/activity" => users::user_activity(req).await,
        "GET /documents/monitoring/v3/activities" => {
            documents_read::monitoring_activities(req).await
        }
        "GET /documents/feasibility/v3" => documents_read::feasibility_v3_get(req).await,
        "GET /documents/monitoring/v3" => documents_read::monitoring_v3_get(req).await,
        "POST /users/account" => users_write::account_post(req).await,
        "POST /users/account/register" => users_write::register(req).await,
        "POST /users/account/verification/resend" => users_write::verification_resend(req).await,
        "GET /users/account/verification/{token}" => users_write::verification(req).await,
        "POST /users/account/login" => users_write::login(req).await,
        "POST /users/account/forgot-password" => users_write::forgot_password(req).await,
        "POST /users/account/reset-password" => users_write::reset_password(req).await,
        "POST /users/account/change-password" => users_write::change_password(req).await,
        "POST /users/account/request" => users_write::account_request(req).await,
        "POST /projects/bind" => projects_write::bind(req).await,
        "POST /projects" => projects_write::save(req).await,
        "PUT /projects" => projects_write::save(req).await,
        "PATCH /projects/privacy" => projects_write::privacy(req).await,
        "DELETE /projects" => projects_write::delete(req).await,
        "POST /geos/polygon" => geo_write::polygon_post(req).await,
        "POST /geos/map/explorer" => geo_write::map_explorer_post(req).await,
        "POST /geos/map/location" => geo_write::map_location_post(req).await,
        "POST /geos/map/location-search" => geo_write::map_location_search_post(req).await,
        "POST /geos/map/upload" => geo_write::map_upload_post(req).await,
        "GET /geos/feature/threat" => feature::threat(req).await,
        "POST /geos/feature/benefit" => feature::benefit(req).await,
        "POST /geos/feature/benefit/people" => feature::benefit_people(req).await,
        "GET /geos/feature/site-characterisation" => feature::site_characterisation(req).await,
        "GET /geos/feature/analysis" => feature::analysis(req).await,
        "GET /geos/feature/pathway" => feature::pathway(req).await,
        "POST /masters/organization" => masters_write::organization_save(req).await,
        "PUT /masters/organization" => masters_write::organization_save(req).await,
        "GET /loggers/list" => masters_write::logger_list(req).await,
        "GET /documents/docx" => documents_write::docx_get(req).await,
        "GET /documents/download/docx" => documents_generate::download_docx(req).await,
        "GET /documents/download/xlsx" => documents_generate::download_xlsx(req).await,
        "GET /documents/download/csv" => documents_generate::download_csv(req).await,
        "GET /documents/geopdf" => documents_generate::geopdf(req).await,
        "DELETE /documents/docx" => documents_write::docx_delete(req).await,
        "GET /documents/ccb" => documents_write::ccb_get(req).await,
        "POST /documents/ccb" => documents_write::ccb_post(req).await,
        "POST /documents/feasibility/v3" => documents_write::feasibility_v3_post(req).await,
        "POST /documents/monitoring/v3" => documents_write::monitoring_v3_post(req).await,
        "POST /loggers/report-problem" => ops::report_problem(req).await,
        "POST /incubators/review" => ops::incubator_review(req).await,
        "GET /users/ga4-report" => ops::ga4_report(req).await,
        "GET /documents/cleanup" => ops::cleanup(req).await,
        "GET /documents/cleanup/reminder" => ops::cleanup_reminder(req).await,
        "*" => not_found::any(req).await,
        // INFALLIBLE: rust/app/src/routes.rs only routes names listed above (its test checks the
        // table against MAP.md; a name without an arm here fails the first replay).
        other => unreachable!("route `{other}` is wired but has no handler"),
    };
    core::cors(core::settle_vary(response, &loaded), origin.as_ref())
}

/// Compile-time guard: the server needs a `Send` future from every handler.
#[allow(dead_code)]
fn assert_send(req: Request) -> impl std::future::Future<Output = Response> + Send {
    handle("x", req)
}
