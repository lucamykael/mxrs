//! The web directory is a trusted build artifact, never a live upload area.
//! Snapshotting it before accepting requests prevents later filesystem changes
//! (including symlink swaps) from turning a public asset URL into a file reader.
//! The directory and its parents must not be writable by untrusted users while
//! this startup snapshot is being built. Hidden entries are never published.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path};
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::{HttpError, Result, error_response};

const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Snapshot(Arc<BTreeMap<String, Bytes>>);

impl Snapshot {
    pub(crate) fn load(root: &Path) -> Result<Self> {
        if !root.is_dir() {
            return Err(HttpError::MissingWebRoot(root.display().to_string()));
        }
        reject_symlink_components(root)?;
        let mut files = BTreeMap::new();
        collect(root, "", &mut files, &mut 0, MAX_SNAPSHOT_BYTES)?;
        Ok(Self(Arc::new(files)))
    }

    pub(crate) async fn respond(self, request: Request) -> Response {
        let Some(path) = public_path(request.uri().path()) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if path == "/api" || path.starts_with("/api/") {
            return error_response(
                StatusCode::NOT_FOUND,
                "unknown_endpoint",
                "unknown API endpoint",
            );
        }
        if !matches!(request.method(), &Method::GET | &Method::HEAD) {
            return (
                StatusCode::METHOD_NOT_ALLOWED,
                [(header::ALLOW, "GET, HEAD")],
            )
                .into_response();
        }
        let key = if path.ends_with('/') {
            format!("{path}index.html")
        } else {
            path
        };
        let Some(bytes) = self.0.get(&key) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let body = if request.method() == Method::HEAD {
            Body::empty()
        } else {
            Body::from(bytes.clone())
        };
        let mut response = body.into_response();
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, content_type(&key).parse().unwrap());
        headers.insert(header::CONTENT_LENGTH, bytes.len().into());
        headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
        headers.insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
        response
    }
}

fn reject_symlink_components(path: &Path) -> Result<()> {
    let mut current = std::path::PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(HttpError::UnsafeWebAsset(path.display().to_string()));
        }
        current.push(component.as_os_str());
        if std::fs::symlink_metadata(&current)
            .map_err(HttpError::WebAssets)?
            .file_type()
            .is_symlink()
        {
            return Err(HttpError::UnsafeWebAsset(current.display().to_string()));
        }
    }
    Ok(())
}

fn collect(
    root: &Path,
    prefix: &str,
    files: &mut BTreeMap<String, Bytes>,
    size: &mut usize,
    limit: usize,
) -> Result<()> {
    for entry in std::fs::read_dir(root).map_err(HttpError::WebAssets)? {
        let entry = entry.map_err(HttpError::WebAssets)?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| HttpError::UnsafeWebAsset(entry.path().display().to_string()))?;
        if name.starts_with('.') {
            continue;
        }
        if !public_component(name) {
            return Err(HttpError::UnsafeWebAsset(
                entry.path().display().to_string(),
            ));
        }
        let kind = entry.file_type().map_err(HttpError::WebAssets)?;
        let key = format!("{prefix}/{name}");
        if kind.is_dir() {
            collect(&entry.path(), &key, files, size, limit)?;
        } else if kind.is_file() {
            let mut bytes = Vec::new();
            std::fs::File::open(entry.path())
                .map_err(HttpError::WebAssets)?
                .take((limit - *size + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(HttpError::WebAssets)?;
            *size += bytes.len();
            if *size > limit {
                return Err(HttpError::WebAssetsTooLarge(limit));
            }
            files.insert(key, bytes.into());
        } else {
            return Err(HttpError::UnsafeWebAsset(
                entry.path().display().to_string(),
            ));
        }
    }
    Ok(())
}

fn public_component(component: &str) -> bool {
    !component.is_empty()
        && !component.starts_with('.')
        && !component.ends_with('.')
        && !component
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\' | ':' | '%'))
}

fn public_path(path: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        decoded.push(if byte == b'%' {
            let high = (bytes.next()? as char).to_digit(16)?;
            let low = (bytes.next()? as char).to_digit(16)?;
            let byte = (high * 16 + low) as u8;
            if matches!(byte, b'/' | b'\\') {
                return None;
            }
            byte
        } else {
            byte
        });
    }
    let decoded = String::from_utf8(decoded).ok()?;
    let inner = decoded
        .strip_prefix('/')?
        .strip_suffix('/')
        .unwrap_or(&decoded[1..]);
    if !inner.is_empty() && !inner.split('/').all(public_component) {
        return None;
    }
    Some(decoded)
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt as _;

    #[test]
    fn public_paths_decode_names_without_admitting_traversal_or_hidden_files() {
        for (path, expected) in [
            ("/", "/"),
            ("/assets/app.js", "/assets/app.js"),
            ("/nested/", "/nested/"),
            ("/hello%20world.json", "/hello world.json"),
            ("/%C3%A9.json", "/é.json"),
        ] {
            assert_eq!(public_path(path).as_deref(), Some(expected));
        }
        for path in [
            "relative",
            "/.env",
            "/.git/config",
            "/../secret",
            "/%2e%2e/secret",
            "/%2fsecret",
            "/%5csecret",
            "/foo\\bar",
            "/foo:bar",
            "/trailing.",
            "/foo//bar",
            "/%00",
            "/%",
            "/%0",
            "/%GG",
            "/%FF",
            "/%252eenv",
        ] {
            assert!(public_path(path).is_none(), "accepted {path}");
        }
    }

    #[tokio::test]
    async fn snapshots_preserve_public_bytes_and_never_expose_later_or_hidden_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("assets")).unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        std::fs::write(root.path().join(".git/config"), "secret").unwrap();
        std::fs::write(root.path().join(".env"), "secret").unwrap();
        std::fs::write(root.path().join("index.html"), "original").unwrap();
        std::fs::write(root.path().join("assets/a.js"), "javascript").unwrap();
        let snapshot = Snapshot::load(root.path()).unwrap();
        std::fs::write(root.path().join("index.html"), "replaced").unwrap();
        std::fs::write(root.path().join("later.txt"), "new").unwrap();
        let index = snapshot
            .clone()
            .respond(Request::get("/").body(Body::empty()).unwrap())
            .await;
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(
            index.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(index.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(
            index.into_body().collect().await.unwrap().to_bytes(),
            "original"
        );
        let head = snapshot
            .clone()
            .respond(Request::head("/assets/a.js").body(Body::empty()).unwrap())
            .await;
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "10");
        assert_eq!(
            head.headers()[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        assert!(
            head.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty()
        );
        for path in [
            "/.env",
            "/.git/config",
            "/later.txt",
            "/missing",
            "/%2e%2e/.env",
            "/api/missing",
            "/api",
        ] {
            let response = snapshot
                .clone()
                .respond(Request::get(path).body(Body::empty()).unwrap())
                .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        let response = snapshot
            .respond(Request::post("/").body(Body::empty()).unwrap())
            .await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()[header::ALLOW], "GET, HEAD");
    }

    #[test]
    fn snapshot_size_limits_fail_instead_of_publishing_a_partial_site() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), [0; 9]).unwrap();
        let mut files = BTreeMap::new();
        assert!(matches!(
            collect(root.path(), "", &mut files, &mut 0, 8),
            Err(HttpError::WebAssetsTooLarge(8))
        ));
        assert!(files.is_empty());
        collect(root.path(), "", &mut files, &mut 0, 9).unwrap();
        assert_eq!(files["/file"].len(), 9);
        assert!(matches!(
            collect(&root.path().join("missing"), "", &mut files, &mut 0, 9),
            Err(HttpError::WebAssets(_))
        ));
        assert!(matches!(
            Snapshot::load(&root.path().join("file")),
            Err(HttpError::MissingWebRoot(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn snapshots_reject_symlink_files_directories_roots_and_parent_traversal() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let public = root.path().join("public");
        std::fs::create_dir(&public).unwrap();
        std::fs::write(root.path().join("secret"), "secret").unwrap();
        symlink(root.path().join("secret"), public.join("leak")).unwrap();
        assert!(matches!(
            Snapshot::load(&public),
            Err(HttpError::UnsafeWebAsset(_))
        ));
        std::fs::remove_file(public.join("leak")).unwrap();
        symlink(root.path(), public.join("cycle")).unwrap();
        assert!(matches!(
            Snapshot::load(&public),
            Err(HttpError::UnsafeWebAsset(_))
        ));
        std::fs::remove_file(public.join("cycle")).unwrap();
        symlink(&public, root.path().join("alias")).unwrap();
        assert!(matches!(
            Snapshot::load(&root.path().join("alias")),
            Err(HttpError::UnsafeWebAsset(_))
        ));
        assert!(matches!(
            Snapshot::load(&public.join("..")),
            Err(HttpError::UnsafeWebAsset(_))
        ));
        std::fs::write(public.join("bad:name"), "content").unwrap();
        assert!(matches!(
            Snapshot::load(&public),
            Err(HttpError::UnsafeWebAsset(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn later_symlink_swaps_cannot_redirect_public_files_to_private_data() {
        let root = tempfile::tempdir().unwrap();
        let public = root.path().join("public");
        std::fs::create_dir(&public).unwrap();
        std::fs::write(public.join("index.html"), "public").unwrap();
        std::fs::write(root.path().join("secret"), "private").unwrap();
        let snapshot = Snapshot::load(&public).unwrap();
        std::fs::remove_file(public.join("index.html")).unwrap();
        std::os::unix::fs::symlink(root.path().join("secret"), public.join("index.html")).unwrap();
        let response = snapshot
            .respond(Request::get("/").body(Body::empty()).unwrap())
            .await;
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "public"
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_asset_names_are_rejected_without_lossy_name_collisions() {
        use std::os::unix::ffi::OsStringExt as _;
        let root = tempfile::tempdir().unwrap();
        let name = std::ffi::OsString::from_vec(vec![0xff]);
        std::fs::write(root.path().join(name), "bytes").unwrap();
        assert!(matches!(
            Snapshot::load(root.path()),
            Err(HttpError::UnsafeWebAsset(_))
        ));
    }

    #[test]
    fn supported_web_formats_have_non_sniffed_content_types() {
        for (extension, expected) in [
            ("html", "text/html; charset=utf-8"),
            ("js", "text/javascript; charset=utf-8"),
            ("mjs", "text/javascript; charset=utf-8"),
            ("css", "text/css; charset=utf-8"),
            ("ttf", "font/ttf"),
            ("json", "application/json"),
            ("map", "application/json"),
            ("svg", "image/svg+xml"),
            ("png", "image/png"),
            ("jpg", "image/jpeg"),
            ("jpeg", "image/jpeg"),
            ("gif", "image/gif"),
            ("webp", "image/webp"),
            ("ico", "image/x-icon"),
            ("woff", "font/woff"),
            ("woff2", "font/woff2"),
            ("wasm", "application/wasm"),
            ("unknown", "application/octet-stream"),
        ] {
            assert_eq!(content_type(&format!("asset.{extension}")), expected);
        }
    }
}
