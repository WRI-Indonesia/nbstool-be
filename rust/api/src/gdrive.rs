//! `utils/cloud_gdrive.CloudDrive` (google-api-python-client 2.143, Drive v3) over REST: the
//! same requests, in the same order, with the same bodies (proven against the client library's
//! own requests through a mock transport: `rust/app-cases/proof/drive_wire.py`, test below).

use serde_json::{Value, json};

use crate::users_write::{google_access_token_for, http};

/// The scopes `build('drive', 'v3')` asks Application Default Credentials for (the discovery
/// document's, in its order).
const DRIVE_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/drive",
    "https://www.googleapis.com/auth/drive.appdata",
    "https://www.googleapis.com/auth/drive.apps.readonly",
    "https://www.googleapis.com/auth/drive.file",
    "https://www.googleapis.com/auth/drive.meet.readonly",
    "https://www.googleapis.com/auth/drive.metadata",
    "https://www.googleapis.com/auth/drive.metadata.readonly",
    "https://www.googleapis.com/auth/drive.photos.readonly",
    "https://www.googleapis.com/auth/drive.readonly",
    "https://www.googleapis.com/auth/drive.scripts",
];

const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

pub struct Drive {
    client: reqwest::Client,
    /// `https://www.googleapis.com/` (the discovery rootUrl); a local server in the test
    root: String,
    token: Option<String>,
}

/// googleapiclient's `HttpError` text: `<HttpError 404 when requesting <uri> returned "<reason>">`.
// shortcut: the `. Details: "..."` part (Python's repr of the error's details list) is left out.
async fn checked(
    uri: &str,
    response: reqwest::Response,
) -> Result<(reqwest::header::HeaderMap, Value), String> {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !status.is_success() {
        let reason = v
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| status.canonical_reason().unwrap_or("").to_string());
        return Err(format!(
            "<HttpError {} when requesting {uri} returned \"{}\">",
            status.as_u16(),
            reason.trim()
        ));
    }
    Ok((headers, v))
}

impl Drive {
    /// `CloudDrive()`: ADC with the Drive scopes.
    pub async fn new() -> Result<Drive, String> {
        let client = http()?;
        let token = google_access_token_for(&client, Some(DRIVE_SCOPES)).await?;
        Ok(Drive {
            client,
            root: "https://www.googleapis.com/".into(),
            token: Some(token),
        })
    }

    async fn post_json(
        &self,
        uri: String,
        body: &Value,
        extra: &[(&str, String)],
    ) -> Result<(reqwest::header::HeaderMap, Value), String> {
        let mut request = self
            .client
            .post(&uri)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(crate::core::py_json_dumps(body));
        for (name, value) in extra {
            request = request.header(*name, value);
        }
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        checked(&uri, request.send().await.map_err(|e| e.to_string())?).await
    }

    /// `create_folder` then `create_anyone_permission`: the folder as `{id, webViewLink}`.
    pub async fn create_anyone_folder(
        &self,
        name: &str,
        parents_id: &str,
    ) -> Result<Value, String> {
        let body = json!({
            "name": name,
            "mimeType": "application/vnd.google-apps.folder",
            "parents": [parents_id],
        });
        let (_, folder) = self
            .post_json(
                format!(
                    "{}drive/v3/files?fields=id%2CwebViewLink&alt=json",
                    self.root
                ),
                &body,
                &[],
            )
            .await?;
        let id = folder.get("id").and_then(Value::as_str).unwrap_or("");
        self.post_json(
            format!(
                "{}drive/v3/files/{}/permissions?alt=json",
                self.root,
                quote(id)
            ),
            &json!({ "type": "anyone", "role": "writer" }),
            &[],
        )
        .await?;
        Ok(folder)
    }

    /// `create_document(path, name, parents_id)`: a resumable upload in one chunk (the file is
    /// read first, as `MediaFileUpload` opens it before the request).
    pub async fn create_document(
        &self,
        path: &str,
        name: &str,
        parents_id: &str,
    ) -> Result<String, String> {
        let data = std::fs::read(path).map_err(|_| {
            format!(
                "[Errno 2] No such file or directory: {}",
                crate::sa::py_repr_str(path)
            )
        })?;
        let uri = format!(
            "{}upload/drive/v3/files?fields=id&alt=json&uploadType=resumable",
            self.root
        );
        let (headers, _) = self
            .post_json(
                uri,
                &json!({ "name": name, "parents": [parents_id] }),
                &[
                    ("x-upload-content-type", DOCX.to_string()),
                    ("x-upload-content-length", data.len().to_string()),
                ],
            )
            .await?;
        let session = headers
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or("ResumableUploadError: no location in the upload answer")?
            .to_string();
        let range = if data.is_empty() {
            "bytes */0".to_string()
        } else {
            format!("bytes 0-{}/{}", data.len() - 1, data.len())
        };
        let mut request = self
            .client
            .put(&session)
            .header(reqwest::header::CONTENT_RANGE, range)
            .body(data);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let (_, file) = checked(&session, request.send().await.map_err(|e| e.to_string())?).await?;
        Ok(file
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string())
    }
}

/// uritemplate's path expansion of `{fileId}`: unreserved characters kept.
fn quote(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// The Rust client's requests equal the ones google-api-python-client sends
    /// (`proof/drive_wire.py` output for the same inputs).
    #[tokio::test]
    async fn requests_match_the_python_client() {
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let log = seen.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let root = format!("http://{}/", listener.local_addr().unwrap());
        let session = format!("{root}upload-session-1");
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let log = log.clone();
            let session = session.clone();
            async move {
                let (parts, body) = req.into_parts();
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                let mut headers = serde_json::Map::new();
                for name in [
                    "content-type",
                    "content-length",
                    "content-range",
                    "x-upload-content-type",
                    "x-upload-content-length",
                ] {
                    if let Some(v) = parts.headers.get(name) {
                        headers.insert(name.into(), json!(v.to_str().unwrap()));
                    }
                }
                let n = {
                    let mut log = log.lock().unwrap();
                    log.push(json!({
                        "method": parts.method.as_str(),
                        "uri": parts.uri.to_string(),
                        "headers": headers,
                        "body": body.iter().map(|b| *b as char).collect::<String>(),
                    }));
                    log.len()
                };
                let mut response = axum::response::Response::new(axum::body::Body::from(match n {
                    1 => r#"{"id": "F1", "webViewLink": "https://drive.example/F1"}"#,
                    2 => r#"{"id": "anyoneWithLink"}"#,
                    3 => "",
                    _ => r#"{"id": "D1"}"#,
                }));
                if n == 3 {
                    response
                        .headers_mut()
                        .insert("location", session.parse().unwrap());
                }
                response
            }
        });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let dir = std::env::temp_dir().join(format!("gdrive-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.docx");
        std::fs::write(&path, b"PK\x03\x04docx-bytes").unwrap();
        let drive = Drive {
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            root: root.clone(),
            token: None,
        };
        let folder = drive
            .create_anyone_folder("[1764300000] Org's Document's", "PARENT1")
            .await
            .unwrap();
        let id = folder
            .get("id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        let file_id = drive
            .create_document(path.to_str().unwrap(), "Feasibility Study", &id)
            .await
            .unwrap();
        std::fs::remove_dir_all(&dir).ok();

        let expected: Vec<Value> = include_str!("../../app-cases/proof/drive_wire.expected.jsonl")
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let got = seen.lock().unwrap().clone();
        assert_eq!(got.len(), 4);
        for (g, e) in got.iter().zip(&expected) {
            // the Python URIs name the real hosts; the paths and queries must be the same
            let e_uri = e["uri"]
                .as_str()
                .unwrap()
                .replace("https://www.googleapis.com/", "/")
                .replace("https://upload.example/session-1", "/upload-session-1");
            assert_eq!(g["method"], e["method"]);
            assert_eq!(g["uri"].as_str().unwrap(), e_uri);
            assert_eq!(g["headers"], e["headers"]);
            assert_eq!(g["body"], e["body"]);
        }
        assert_eq!(json!({"folder": folder, "file_id": file_id}), expected[4]);
    }
}
