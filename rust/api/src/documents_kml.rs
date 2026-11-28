//! `GET /documents/kml`: `documents_get_kml` of `document_apis/routes.py`.

use axum::response::Response;
use sqlx::Row;

use crate::core::{self, Req, arg, ctx};

pub async fn kml(req: Req) -> Response {
    // request.args.get: None when absent; `session_id = NULL` matches no row, as `IS NULL` on a
    // NOT NULL column does not either.
    let session_id = arg(&req, "session_id");
    let pool = &ctx().await.app;

    // UserSessions.find_by_session_id: filter_by(session_id, is_active=1).first()
    let session = match crate::pq::query(
        "select project_name from tbl_user_sessions where session_id = $1 and is_active = 1 limit 1",
    )
    .bind(&session_id)
    .fetch_optional(pool)
    .await
    {
        Ok(row) => row,
        Err(e) => return core::db_error(e).await,
    };
    let Some(session) = session else {
        return core::json(400, &core::app_error("invalid session", None));
    };
    let session_id = session_id.unwrap_or_default();

    // Query.scalar(): one() under the hood, so a second row raises MultipleResultsFound.
    let rows = match crate::pq::query("select ST_AsKML(geom) from polygons where session_id = $1")
        .bind(&session_id)
        .fetch_all(pool)
        .await
    {
        Ok(rows) => rows,
        Err(e) => return core::db_error(e).await,
    };
    if rows.len() > 1 {
        let message = "Multiple rows were found when exactly one was required";
        return core::json(500, &core::other_error(message, None).await);
    }
    let geometry: Option<String> = match rows.first().map(|r| r.try_get(0)) {
        None => None,
        Some(Ok(g)) => g,
        Some(Err(e)) => return core::db_error(e).await,
    };
    let Some(geometry) = geometry.filter(|g| !g.is_empty()) else {
        return core::json(400, &core::app_error("invalid data", None));
    };

    let project_name: Option<String> = session.try_get(0).unwrap_or(None);
    let name = escape(
        project_name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(&session_id),
    );
    core::with_header(
        core::with_header(
            core::respond(
                200,
                "application/vnd.google-earth.kml+xml; charset=utf-8",
                body(&name, &geometry),
            ),
            "content-disposition",
            "attachment; filename=aoi.kml",
        ),
        "cache-control",
        "no-cache",
    )
}

fn body(name: &str, geometry: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <kml xmlns=\"http://www.opengis.net/kml/2.2\"><Document><name>{name}</name>\
         <Placemark><name>{name}</name>{geometry}</Placemark></Document></kml>\n"
    )
}

/// `xml.sax.saxutils.escape`: &, > and < only.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('>', "&gt;")
        .replace('<', "&lt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_matches_saxutils() {
        assert_eq!(escape("a&b<c>\"'"), "a&amp;b&lt;c&gt;\"'");
    }

    fn req(path: &str) -> Req {
        let (parts, _) = axum::http::Request::builder()
            .uri(path)
            .body(())
            .unwrap()
            .into_parts();
        Req {
            parts,
            body: Default::default(),
        }
    }

    /// Every recorded kml case, run against staging (read-only).
    #[tokio::test]
    #[ignore]
    async fn matches_recordings() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../app-cases/http");
        for case in ["kml-missing", "kml-unknown", "kml-0", "kml-1", "kml-2"] {
            let toml = std::fs::read_to_string(format!("{dir}/{case}.toml")).unwrap();
            let path = toml
                .lines()
                .find_map(|l| l.strip_prefix("path = \""))
                .unwrap()
                .trim_end_matches('"');
            let status: u16 = toml
                .lines()
                .find_map(|l| l.strip_prefix("status = "))
                .unwrap()
                .parse()
                .unwrap();
            let resp = kml(req(path)).await;
            assert_eq!(resp.status(), status, "{case}");
            for line in toml.split("[response.headers]").nth(1).unwrap().lines() {
                let Some((k, v)) = line.split_once(" = ") else {
                    continue;
                };
                let v = v.trim_matches('"');
                if v != "[..]" {
                    assert_eq!(
                        resp.headers().get(k).unwrap().to_str().unwrap(),
                        v,
                        "{case} {k}"
                    );
                }
            }
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let recorded = std::fs::read_to_string(format!("{dir}/{case}.body")).unwrap();
            let mut rest = std::str::from_utf8(&body).unwrap();
            for (i, piece) in recorded.split("[..]").enumerate() {
                if i == 0 {
                    assert!(rest.starts_with(piece), "{case}");
                    rest = &rest[piece.len()..];
                } else {
                    let at = rest.find(piece).expect(case);
                    rest = &rest[at + piece.len()..];
                }
            }
            assert!(rest.is_empty(), "{case}");
        }
    }
}
