//! `utils/cloud_storage.CloudStorage` (google-cloud-storage 2.x): every failure is only logged.
//! `STORAGE_EMULATOR_HOST` (anonymous) when set, else storage.googleapis.com with a token.
//! `users_write::gcs_upload` is the multipart upload; these are the other calls.

use crate::users_write::{google_access_token, http};

fn base() -> (String, bool) {
    match std::env::var("STORAGE_EMULATOR_HOST")
        .ok()
        .filter(|h| !h.is_empty())
    {
        Some(host) => (host, true),
        None => ("https://storage.googleapis.com".into(), false),
    }
}

/// `quote(name, safe='')`: everything but `A-Za-z0-9_.-~` escaped.
fn quote(name: &str) -> String {
    let mut out = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

async fn request(method: reqwest::Method, url: String) -> Result<reqwest::Response, String> {
    request_with(method, url, None).await
}

async fn request_with(
    method: reqwest::Method,
    url: String,
    content_type: Option<&str>,
) -> Result<reqwest::Response, String> {
    let client = http()?;
    let (_, emulator) = base();
    let mut request = client.request(method, url);
    if let Some(ct) = content_type {
        request = request.header(reqwest::header::CONTENT_TYPE, ct);
    }
    if !emulator {
        request = request.bearer_auth(google_access_token(&client).await?);
    }
    request.send().await.map_err(|e| e.to_string())
}

/// `CloudStorage().__init__`: `get_bucket` reloads the bucket's metadata.
pub async fn get_bucket() {
    let result: Result<(), String> = async {
        let bucket = std::env::var("GCS_BUCKET_NAME").map_err(|e| e.to_string())?;
        let (base, _) = base();
        let response = request(
            reqwest::Method::GET,
            format!(
                "{base}/storage/v1/b/{}?projection=noAcl&prettyPrint=false",
                quote(&bucket)
            ),
        )
        .await?;
        if !response.status().is_success() {
            return Err(format!("{} GET bucket", response.status().as_u16()));
        }
        Ok(())
    }
    .await;
    if let Err(e) = result {
        eprintln!("CloudStorage init FAILED: {e}");
    }
}

/// `CloudStorage.delete(target)`.
pub async fn delete(target: &str) {
    let result: Result<(), String> = async {
        let bucket = std::env::var("GCS_BUCKET_NAME").map_err(|e| e.to_string())?;
        let (base, _) = base();
        let response = request(
            reqwest::Method::DELETE,
            format!(
                "{base}/storage/v1/b/{}/o/{}?prettyPrint=false",
                quote(&bucket),
                quote(target)
            ),
        )
        .await?;
        if !response.status().is_success() {
            return Err(format!("{} DELETE {target}", response.status().as_u16()));
        }
        Ok(())
    }
    .await;
    if let Err(e) = result {
        eprintln!("CloudStorage Delete FAILED: {target} | {e}");
    }
}

/// `CloudStorage.download(source)` to the same relative path: `download_to_filename` opens the
/// file first (a missing folder fails before any request) and removes it when the GET fails.
pub async fn download(source: &str) {
    let result: Result<(), String> = async {
        let bucket = std::env::var("GCS_BUCKET_NAME").map_err(|e| e.to_string())?;
        std::fs::File::create(source).map_err(|e| e.to_string())?;
        let (base, _) = base();
        let fetched = async {
            // google-resumable-media's download request carries this header (recorded)
            let response = request_with(
                reqwest::Method::GET,
                format!(
                    "{base}/download/storage/v1/b/{}/o/{}?alt=media",
                    quote(&bucket),
                    quote(source)
                ),
                Some("application/json; charset=UTF-8"),
            )
            .await?;
            if !response.status().is_success() {
                return Err(format!("{} GET {source}", response.status().as_u16()));
            }
            response.bytes().await.map_err(|e| e.to_string())
        }
        .await;
        match fetched {
            Ok(bytes) => std::fs::write(source, &bytes).map_err(|e| e.to_string()),
            Err(e) => {
                let _ = std::fs::remove_file(source);
                Err(e)
            }
        }
    }
    .await;
    if let Err(e) = result {
        eprintln!("CloudStorage Download FAILED: {source} to {source} | {e}");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn quote_is_python_quote_safe_empty() {
        assert_eq!(
            super::quote("generated-file/docx/a b.docx"),
            "generated-file%2Fdocx%2Fa%20b.docx"
        );
    }
}
