//! The `generate` branch of `POST /documents/feasibility/v3` and `/monitoring/v3`:
//! `load_draft` (in the save's transaction), `generate_extra_tags`, `_figures`, `build_context`
//! (nbstool-doccontext), `_generate` (nbstool-docx), `gcs.upload`, the DocumentList row.

use std::path::Path;

use axum::response::Response;
use serde_json::{Value, json};
use sqlx::Row;

use crate::core::{self, Req};

pub enum GenFail {
    App(String),
    Py(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for GenFail {
    fn from(e: sqlx::Error) -> Self {
        GenFail::Db(e)
    }
}

const OUTPUT_FOLDER: &str = "generated-file/docx-v3/";
const FIGURE_WIDTH_MM: f64 = 165.0;

async fn read_analyzer(
    tx: &mut sqlx::PgConnection,
    session_id: &str,
) -> Result<Option<nbstool_doccontext::Analyzer>, sqlx::Error> {
    // `DataAnalyzer.find_by_session_id`: filter_by(...).first(), no ORDER BY
    let row = crate::pq::query(
        "select site_information_json, nature_json, climate_json, people_json, threat_json, \
         intervention_eligibility_json, benefit_json from \"DataAnalyzer\" where session_id = $1 limit 1",
    )
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?;
    Ok(row.map(|row| {
        let col = |i: usize| row.get::<Option<Value>, _>(i).unwrap_or(Value::Null);
        nbstool_doccontext::Analyzer {
            site_information_json: col(0),
            nature_json: col(1),
            climate_json: col(2),
            people_json: col(3),
            threat_json: col(4),
            intervention_eligibility_json: col(5),
            benefit_json: col(6),
        }
    }))
}

/// The `(form, user_input)` of each DocumentData row of `types`, in that order (missing left out).
async fn drafts(
    tx: &mut sqlx::PgConnection,
    session_id: &str,
    types: &[&str],
) -> Result<Vec<(Value, Value)>, sqlx::Error> {
    let mut out = Vec::new();
    for cert in types {
        let row = crate::pq::query(
            "select form, user_input from \"DocumentData\" where session_id = $1 and certification_type = $2 limit 1",
        )
        .bind(session_id)
        .bind(cert)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = row {
            out.push((
                row.get::<Option<Value>, _>(0).unwrap_or(Value::Null),
                row.get::<Option<Value>, _>(1).unwrap_or(Value::Null),
            ));
        }
    }
    Ok(out)
}

pub async fn generate_v3(
    req: &Req,
    mut tx: sqlx::Transaction<'static, sqlx::Postgres>,
    cert: &str,
    session_id: String,
) -> Result<Response, GenFail> {
    let feasibility = cert == "FeasibilityV3";
    let analyzer = read_analyzer(&mut tx, &session_id).await?;
    if feasibility && analyzer.is_none() {
        return Err(GenFail::App("fail, session id Not found".into()));
    }
    // feasibility: merge_form(stored.form, prefill) of the row just saved; monitoring:
    // load_draft(session_id, 'MonitoringV3', ('FeasibilityV3',))
    let types: &[&str] = if feasibility {
        &["FeasibilityV3"]
    } else {
        &["FeasibilityV3", "MonitoringV3"]
    };
    let rows = drafts(&mut tx, &session_id, types).await?;
    let (form, user_input) =
        nbstool_doccontext::load_draft(analyzer.as_ref(), &rows).map_err(GenFail::Py)?;
    if analyzer.is_none() {
        return Err(GenFail::App("fail, session id Not found".into()));
    }

    let today = chrono::Local::now().format("%d %B %Y").to_string();
    let extra = crate::documents_csv::generate_extra_tags_for(req, &session_id, &today)
        .await
        .map_err(GenFail::Py)?;

    // _figures(session_id): the AOI is prepared before the context is built
    let (aoi, settings) = crate::figures::aoi_and_settings(&session_id)
        .await
        .map_err(GenFail::Py)?;
    let mut context =
        nbstool_doccontext::build_context(analyzer.as_ref(), &form, &user_input, &extra)
            .map_err(GenFail::Py)?;

    std::fs::create_dir_all(OUTPUT_FOLDER).map_err(|e| GenFail::Py(e.to_string()))?;
    let stamp = chrono::Local::now().format("%Y%m%d%H%M%S").to_string();
    let suffix = if feasibility {
        "feasibility"
    } else {
        "monitoring"
    };
    let output_path = format!("{OUTPUT_FOLDER}{session_id}-{stamp}-{suffix}.docx");

    let mut figures: Vec<(&str, nbstool_docx::Figure)> = Vec::new();
    // disturbance_map: drawn next to the docx
    let disturbance_path = format!("{OUTPUT_FOLDER}{session_id}-{stamp}-disturbance_map.png");
    let target = disturbance_path.clone();
    let drawn = tokio::task::spawn_blocking(move || {
        nbstool_fig::render_disturbance_map(
            &settings,
            &aoi,
            Path::new(&target),
            Path::new(crate::figures::TILE_CACHE),
        )
    })
    .await
    .map_err(|e| GenFail::Py(e.to_string()))?
    .map_err(|e| GenFail::Py(e.message))?;
    let figure = |path: &str| -> Result<nbstool_docx::Figure, GenFail> {
        Ok(nbstool_docx::Figure {
            filename: Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            png: std::fs::read(path).map_err(|e| GenFail::Py(e.to_string()))?,
            width_emu: nbstool_docx::mm(FIGURE_WIDTH_MM),
        })
    };
    match drawn {
        Some(_) => figures.push(("disturbance_map", figure(&disturbance_path)?)),
        None => context["disturbance_map"] = json!("[Figure: disturbance map]"),
    }
    // boundary_map: redrawn at its per-session path and uploaded
    match crate::figures::generate_boundary_map(&session_id)
        .await
        .map_err(GenFail::Py)?
    {
        Some(path) => figures.push(("boundary_map", figure(&path)?)),
        None => context["boundary_map"] = json!("[Figure: boundary map]"),
    }

    let template = if feasibility {
        "assets/feasibility_v3_template.docx"
    } else {
        "assets/monitoring_v3_template.docx"
    };
    let template = std::fs::read(template).map_err(|e| GenFail::Py(e.to_string()))?;
    let modified = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let docx = tokio::task::spawn_blocking(move || {
        nbstool_docx::render(
            &template,
            &context,
            &nbstool_docx::Options {
                figures: &figures,
                modified_utc: &modified,
            },
        )
    })
    .await
    .map_err(|e| GenFail::Py(e.to_string()))?
    .map_err(|e| GenFail::Py(e.to_string()))?;
    // python-docx's archive: CPython zipfile + zlib
    let docx = crate::pyzip::python_docx_zip(&docx).map_err(GenFail::Py)?;
    std::fs::write(&output_path, &docx).map_err(|e| GenFail::Py(e.to_string()))?;

    crate::users_write::gcs_upload(&output_path, &docx, "docx").await;

    let document_id = uuid::Uuid::new_v4().to_string();
    let document_name = Path::new(&output_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let client_name = if feasibility {
        "Feasibility Study"
    } else {
        "Monitoring Plan"
    };
    crate::pq::query(
        "INSERT INTO \"DocumentList\" (project_id, document_id, document_type, document_status, \
         document_name, client_name, created_at, is_active) VALUES ($1, $2, $3, 'Final', $4, $5, $6, 1)",
    )
    .bind(&session_id)
    .bind(&document_id)
    .bind(cert)
    .bind(&document_name)
    .bind(client_name)
    .bind(core::get_date())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(core::json(
        200,
        &core::success(
            json!({ "result": { "message": "generated", "document_id": document_id, "document_name": document_name } }),
            None,
            None,
        ),
    ))
}

/// The bytes `open(path, 'w')` writes for `text`: the locale encoding and newline translation of
/// the host, as Python does (Windows: cp1252, LF written as CRLF; Linux, staging: UTF-8 as is).
fn python_text_file(text: &str) -> Result<Vec<u8>, GenFail> {
    if !cfg!(windows) {
        return Ok(text.as_bytes().to_vec());
    }
    let text = text.replace('\n', "\r\n");
    for (i, c) in text.chars().enumerate() {
        let mut buf = [0u8; 4];
        if encoding_rs::WINDOWS_1252.encode(c.encode_utf8(&mut buf)).2 {
            return Err(GenFail::Py(format!(
                "'charmap' codec can't encode character '\\u{:04x}' in position {i}: character maps to <undefined>",
                c as u32
            )));
        }
    }
    Ok(encoding_rs::WINDOWS_1252.encode(&text).0.into_owned())
}

/// `render_ccb_document(session_id, document_type)`: the CCB docx and its CSV from the saved
/// sections plus the v3 analysis, written to `generated-file/docx-ccb/<sid>-ccb-templates.docx`
/// and `generated-file/csv/<sid>-ccb-data.csv`, both uploaded.
pub async fn render_ccb_document(session_id: &str, document_type: &str) -> Result<(), GenFail> {
    let app = &crate::core::ctx().await.app;
    let mut conn = app.acquire().await?;
    let row = crate::pq::query(
        "select section_1, section_2, section_3, section_4, section_5 from \"DocumentData\" \
         where session_id = $1 and certification_type = $2 limit 1",
    )
    .bind(session_id)
    .bind(document_type)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Err(GenFail::App("File not found".into()));
    };
    let mut sections = Vec::with_capacity(5);
    for i in 0..5 {
        let bytes: Option<Vec<u8>> = row.try_get(i)?;
        sections.push(match bytes {
            None => Value::Null,
            Some(b) => crate::masters_write::pickle_json(&b).map_err(GenFail::Py)?,
        });
    }
    // UserSessions.find_by_session_id (active rows) -> User
    let user_row = crate::pq::query(
        "select u.name, u.email, u.extended_data from tbl_user_sessions s join tbl_users u on u.id = s.user_id \
         where s.id = (select id from tbl_user_sessions where session_id = $1 and is_active = 1 limit 1)",
    )
    .bind(session_id)
    .fetch_optional(&mut *conn)
    .await?;
    let user = match user_row {
        None => None,
        Some(r) => {
            let extended: Option<Vec<u8>> = r.try_get(2)?;
            Some(nbstool_doccontext::ccb::User {
                name: json!(r.try_get::<Option<String>, _>(0)?),
                email: json!(r.try_get::<Option<String>, _>(1)?),
                extended_data: match extended {
                    None => Value::Null,
                    Some(b) => crate::masters_write::pickle_json(&b).map_err(GenFail::Py)?,
                },
            })
        }
    };
    let analyzer = read_analyzer(&mut conn, session_id).await?;
    let polygon = crate::pq::query(
        "select ST_AsGeoJSON(geom), project_area_size::float8 from polygons where session_id = $1 limit 1",
    )
    .bind(session_id)
    .fetch_optional(&mut *conn)
    .await?;
    drop(conn);
    let (geojson, area): (Option<Option<String>>, Option<f64>) = match &polygon {
        None => (None, None),
        Some(r) => (Some(r.try_get(0)?), r.try_get(1)?),
    };
    let input = nbstool_doccontext::ccb::Input {
        session_id,
        user: user.as_ref(),
        sections: [
            &sections[0],
            &sections[1],
            &sections[2],
            &sections[3],
            &sections[4],
        ],
        analyzer: analyzer.as_ref(),
        geojson: geojson.as_ref().map(|g| g.as_deref()),
        polygon_area: area,
        current_year: chrono::Datelike::year(&chrono::Local::now()),
    };
    let data = nbstool_doccontext::ccb::render_data(&input).map_err(GenFail::Py)?;

    // generate_document_form: the CSV, the docx, then both uploads
    for folder in ["generated-file/csv/", "generated-file/docx-ccb/"] {
        std::fs::create_dir_all(folder).map_err(|e| GenFail::Py(e.to_string()))?;
    }
    let csv_path = format!("generated-file/csv/{session_id}-ccb-data.csv");
    let csv = python_text_file(&nbstool_doccontext::ccb::csv(&data))?;
    std::fs::write(&csv_path, &csv).map_err(|e| GenFail::Py(e.to_string()))?;
    let docx_path = format!("generated-file/docx-ccb/{session_id}-ccb-templates.docx");
    let template =
        std::fs::read("assets/ccb_template.docx").map_err(|e| GenFail::Py(e.to_string()))?;
    let modified = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let docx = tokio::task::spawn_blocking(move || {
        nbstool_docx::render(
            &template,
            &data,
            &nbstool_docx::Options {
                figures: &[],
                modified_utc: &modified,
            },
        )
    })
    .await
    .map_err(|e| GenFail::Py(e.to_string()))?
    .map_err(|e| GenFail::Py(e.to_string()))?;
    // python-docx's archive: CPython zipfile + zlib
    let docx = crate::pyzip::python_docx_zip(&docx).map_err(GenFail::Py)?;
    std::fs::write(&docx_path, &docx).map_err(|e| GenFail::Py(e.to_string()))?;
    crate::users_write::gcs_upload(&csv_path, &csv, "csv").await;
    crate::users_write::gcs_upload(&docx_path, &docx, "docx").await;
    Ok(())
}

// ---- GET /documents/download/{xlsx,csv,docx} (download/routes.py; no activity log) ----

async fn download_fail(e: GenFail) -> Response {
    match e {
        GenFail::App(m) => core::json(400, &core::app_error(&m, None)),
        GenFail::Py(m) => core::json(500, &core::other_error(&m, None).await),
        GenFail::Db(e) => core::db_error(e).await,
    }
}

const STILL_GENERATING: &str = "we are still generating your file, please wait a minute";

/// `/download/xlsx` and `/download/csv`: the session's file from GCS, sent as an attachment.
async fn download_session_file(req: &Req, folder: &str, suffix: &str) -> Response {
    let run = async {
        let Some(session_id) = core::arg(req, "session_id") else {
            return Err(GenFail::Py(
                "can only concatenate str (not \"NoneType\") to str".into(),
            ));
        };
        let path = format!("{folder}{session_id}{suffix}");
        crate::gcs::download(&path).await;
        if std::fs::metadata(&path).is_err() {
            return Err(GenFail::App(STILL_GENERATING.into()));
        }
        crate::figures::send_file(&path, None, true, None).map_err(|e| GenFail::Py(e.to_string()))
    };
    match run.await {
        Ok(response) => response,
        Err(e) => download_fail(e).await,
    }
}

pub async fn download_xlsx(req: Req) -> Response {
    download_session_file(&req, "generated-file/xlsx/", "-excel-format.xlsx").await
}

pub async fn download_csv(req: Req) -> Response {
    download_session_file(&req, "generated-file/csv/", "-data.csv").await
}

/// `/download/docx`: a DocumentList file under its client name (a CCB document is rendered now).
pub async fn download_docx(req: Req) -> Response {
    let run = async {
        let document_id = core::arg(&req, "document_id");
        let row = crate::pq::query(
            "select project_id, document_type, document_name from \"DocumentList\" \
             where (document_id = $1 or ($1::text is null and document_id is null)) and is_active = 1 limit 1",
        )
        .bind(&document_id)
        .fetch_optional(&crate::core::ctx().await.app)
        .await?;
        let Some(row) = row else {
            return Err(GenFail::App(STILL_GENERATING.into()));
        };
        let (project_id, document_type, document_name): (String, String, String) =
            (row.try_get(0)?, row.try_get(1)?, row.try_get(2)?);
        let ccb = document_type.to_lowercase() == "ccb";
        let (folder, client_name) = if ccb {
            ("generated-file/docx-ccb/", "CCB Project Documentation.docx")
        } else if document_type == "FeasibilityV3" {
            ("generated-file/docx-v3/", "Feasibility Study.docx")
        } else if document_type == "MonitoringV3" {
            ("generated-file/docx-v3/", "Monitoring Plan.docx")
        } else {
            (
                "generated-file/docx/",
                "Preliminary Assessment Document.docx",
            )
        };
        let path = format!("{folder}{document_name}");
        if ccb {
            render_ccb_document(&project_id, &document_type).await?;
        } else {
            crate::gcs::download(&path).await;
        }
        if !std::fs::metadata(&path).is_ok_and(|m| m.len() > 0) {
            return Err(GenFail::App(STILL_GENERATING.into()));
        }
        crate::figures::send_file(
            &path,
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
            true,
            Some(client_name),
        )
        .map_err(|e| GenFail::Py(e.to_string()))
    };
    match run.await {
        Ok(response) => response,
        Err(e) => download_fail(e).await,
    }
}

// ---- GET /documents/geopdf (document_apis/routes.py documents_get_geopdf; not logged) ----

/// basemap-data's `shadedrelief.jpg` (Basemap.shadedrelief): shipped next to the binary, or in
/// `assets/`.
fn shadedrelief() -> std::path::PathBuf {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("shadedrelief.jpg")));
    match beside {
        Some(p) if p.exists() => p,
        _ => std::path::PathBuf::from("assets/shadedrelief.jpg"),
    }
}

pub async fn geopdf(req: Req) -> Response {
    let run = async {
        let session_id = core::arg(&req, "session_id");
        let Some(layers) = core::arg(&req, "layers") else {
            return Err(GenFail::Py(
                "'NoneType' object has no attribute 'split'".into(),
            ));
        };
        let layers: Vec<String> = layers.split(',').map(str::to_string).collect();
        let title = core::arg(&req, "title").unwrap_or_default();
        let app = &crate::core::ctx().await.app;
        // UserSessions.find_by_session_id (active rows), then Polygons.find_by_session_id
        let known: Option<String> = crate::pq::query_scalar(
            "select session_id from tbl_user_sessions where (session_id = $1 or ($1::text is null and session_id is null)) \
             and is_active = 1 limit 1",
        )
        .bind(&session_id)
        .fetch_optional(app)
        .await?;
        let Some(known) = known else {
            return Err(GenFail::App("invalid session".into()));
        };
        let geojson: Option<Option<String>> = crate::pq::query_scalar(
            "select ST_AsGeoJSON(geom) from polygons where session_id = $1 limit 1",
        )
        .bind(&known)
        .fetch_optional(app)
        .await?;
        let Some(geojson) = geojson else {
            return Err(GenFail::App("invalid data".into()));
        };
        let geojson = geojson
            .ok_or_else(|| GenFail::Py("'NoneType' object has no attribute 'srid'".into()))?;
        let base = match crate::layers::func_get_layers_v2().await {
            Ok(list) => Value::Array(list),
            Err(response) => return Ok(response),
        };
        crate::figures::init_gdal();
        let pdf = tokio::task::spawn_blocking(move || {
            let (work, tiles, relief) = (
                Path::new("generated-geopdf"),
                Path::new(crate::figures::TILE_CACHE),
                shadedrelief(),
            );
            let env = nbstool_fig::geopdf::GeoPdfEnv {
                work_root: work,
                tile_cache: tiles,
                shadedrelief: &relief,
                http_cache: None,
                now_utc: None,
            };
            nbstool_fig::geopdf::generate_geopdf(&geojson, &layers, &base, &title, &env)
        })
        .await
        .map_err(|e| GenFail::Py(e.to_string()))?
        .map_err(|e| GenFail::Py(e.message))?;
        // send_file(BytesIO, as_attachment=True, download_name='geo.pdf'): no ETag / Last-Modified
        let response = core::respond(200, "application/pdf", pdf);
        let response = core::with_header(
            response,
            "content-disposition",
            "attachment; filename=geo.pdf",
        );
        Ok(core::with_header(response, "cache-control", "no-cache"))
    };
    match run.await {
        Ok(response) => response,
        Err(e) => download_fail(e).await,
    }
}
