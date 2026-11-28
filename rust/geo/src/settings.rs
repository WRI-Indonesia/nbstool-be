//! `v3/settings.py`: the raster root from tbl_master_settings, and `layer_path`.
//!
//! The Python caches each row for one request (`refresh()` runs before every request). Here a
//! `Settings` is read once per request with `Settings::load` and passed down, which is the same
//! reach: one query per request, a changed row is seen by the next request. A missing V3_BUCKET
//! row raises at the first `layer_path`, as `get_setting` does, not at load.

use sqlx::PgPool;

use crate::config::{
    AOH_MAX_WORKERS, AOH_MAX_WORKERS_SETTING, AOH_RASTER_ROOT, AOH_RASTER_ROOT_SETTING,
    V3_BUCKET_SETTING,
};
use crate::{Error, Result};

const SETTINGS_TABLE: &str = "tbl_master_settings";

#[derive(Clone, Debug)]
pub struct Settings {
    v3_bucket: Option<String>,
    aoh_root: Option<String>,
    aoh_max_workers: Option<String>,
}

impl Settings {
    /// Both rows from the application database (DB_SQLALCHEMY_URI's database).
    pub async fn load(app_db: &PgPool) -> Result<Self> {
        Ok(Self {
            v3_bucket: fetch(app_db, V3_BUCKET_SETTING).await?,
            aoh_root: fetch(app_db, AOH_RASTER_ROOT_SETTING).await?,
            aoh_max_workers: fetch(app_db, AOH_MAX_WORKERS_SETTING).await?,
        })
    }

    /// Fixed values, for runs without a database (tests, scripts).
    pub fn fixed(v3_bucket: &str, aoh_root: Option<&str>) -> Self {
        Self {
            v3_bucket: Some(v3_bucket.to_string()),
            aoh_root: aoh_root.map(str::to_string),
            aoh_max_workers: None,
        }
    }

    /// `int(get_setting(AOH_MAX_WORKERS_SETTING, default=str(AOH_MAX_WORKERS)))`
    pub fn aoh_max_workers(&self) -> Result<usize> {
        match self.aoh_max_workers.as_deref() {
            None => Ok(AOH_MAX_WORKERS),
            Some(v) => v
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|n| *n > 0)
                .map(|n| n as usize)
                .ok_or_else(|| {
                    // int() of a non-number, or ThreadPoolExecutor(max_workers <= 0)
                    Error::new(
                        "ValueError",
                        format!("invalid literal for int() with base 10: '{v}'"),
                    )
                }),
        }
    }

    /// `get_setting(V3_BUCKET)`: required.
    fn v3_bucket(&self) -> Result<&str> {
        self.v3_bucket.as_deref().ok_or_else(|| {
            Error::new(
                "RuntimeError",
                format!(
                    "{SETTINGS_TABLE} has no usable '{V3_BUCKET_SETTING}' row. Insert it before running \
                     the v3 site characterisation."
                ),
            )
        })
    }

    /// `settings.layer_path`, with the same rooting rules.
    pub fn layer_path(&self, layer: &str) -> Result<String> {
        let mut layer = layer.to_string();
        if layer == AOH_RASTER_ROOT || layer.starts_with(&format!("{AOH_RASTER_ROOT}/")) {
            let root = self
                .aoh_root
                .as_deref()
                .unwrap_or(AOH_RASTER_ROOT)
                .trim_end_matches('/');
            layer = format!("{root}{}", &layer[AOH_RASTER_ROOT.len()..]);
        }
        if layer.starts_with("http") || layer.starts_with("/vsi") {
            return Ok(layer);
        }
        let root = self.v3_bucket()?.trim_end_matches('/');
        Ok(if root.starts_with("http") {
            format!("{root}/{layer}")
        } else {
            format!("{root}/v3_{layer}")
        })
    }
}

/// One row by name; an absent row or an empty value is `None` (Python: `not row[0]`).
async fn fetch(db: &PgPool, name: &str) -> Result<Option<String>> {
    // simple protocol with the name inlined as psycopg2 would (crate::db::pg_literal)
    let query = format!(
        "select value from {SETTINGS_TABLE} where name = {}",
        crate::db::pg_literal(name)?
    );
    let row = sqlx::raw_sql(&query)
        .fetch_all(db)
        .await
        .and_then(|rows| {
            rows.first()
                .map(|r| sqlx::Row::try_get::<Option<String>, _>(r, 0))
                .transpose()
        })
        // Only the error kind: a driver error can carry connection details.
        .map_err(|_| Error::new("OperationalError", "settings query failed"))?;
    Ok(row.flatten().filter(|v| !v.is_empty()))
}

/// The path GDAL opens for a layer path, as rasterio's `_parse_path` turns an http(s) URL into
/// a /vsicurl path.
pub fn gdal_path(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        format!("/vsicurl/{path}")
    } else {
        path.to_string()
    }
}

/// A pool on the database a SQLAlchemy URI names (`postgresql+psycopg2://` is accepted). The URI
/// is never logged; a parse failure reports only that it failed.
pub fn pool_from_uri(uri: &str) -> Result<PgPool> {
    let uri = uri.replacen("postgresql+psycopg2://", "postgresql://", 1);
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&uri)
        .map_err(|_| Error::new("ArgumentError", "could not parse the database URI"))
}
