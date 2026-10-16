//! Embedded static assets with content-hashed names (spec 010).

use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(RustEmbed)]
#[folder = "static/"]
pub struct Assets;

struct Index {
    /// logical name → hashed name
    hashed: HashMap<String, String>,
    /// hashed name → logical name
    logical: HashMap<String, String>,
}

fn index() -> &'static Index {
    static INDEX: OnceLock<Index> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut hashed = HashMap::new();
        let mut logical = HashMap::new();
        for name in Assets::iter() {
            let file = Assets::get(&name).expect("listed asset exists");
            let hash = crate::domain::hex(&file.metadata.sha256_hash()[..4]);
            let h = match name.rsplit_once('.') {
                Some((stem, ext)) => format!("{stem}.{hash}.{ext}"),
                None => format!("{name}.{hash}"),
            };
            hashed.insert(name.to_string(), h.clone());
            logical.insert(h, name.to_string());
        }
        Index { hashed, logical }
    })
}

/// URL for a logical asset name, e.g. `url("app.css")` → `/static/app.1a2b3c4d.css`.
pub fn url(name: &str) -> String {
    match index().hashed.get(name) {
        Some(h) => format!("/static/{h}"),
        None => format!("/static/{name}"),
    }
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

pub async fn serve(Path(file): Path<String>) -> Response {
    let idx = index();
    let (name, immutable) = match idx.logical.get(&file) {
        Some(n) => (n.clone(), true),
        None => (file.clone(), false),
    };
    let Some(data) = Assets::get(&name) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let cache = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=300"
    };
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static(content_type(&name)),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
        ],
        data.data.into_owned(),
    )
        .into_response()
}
