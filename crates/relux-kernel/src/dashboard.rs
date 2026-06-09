//! Static serving for the standalone Relux dashboard shell (`relux-kernel serve`).
//!
//! The Relux MVP product boots from one command and opens a usable dashboard
//! served by `relux-kernel` itself - no dependency on the old Relix web bridge,
//! no login/401/token for the local developer product (`docs/RELUX_MASTER_PLAN.md`
//! section 11 Dashboard, section 14 MVP). This module resolves the committed Vite bundle
//! and turns one request path into either a real bundle file or the SPA's
//! `index.html` (history fallback), with honest content types and a clean 503
//! when the bundle has not been built.
//!
//! It reuses the EXISTING committed artifact at
//! `crates/relix-web-bridge/dashboard-dist` (the dashboard's `npm run build`
//! output) instead of introducing a second dist path. The Vite build uses
//! `base: "/dashboard/"`, so every asset URL is `/dashboard/assets/...`; this
//! module serves those under the same prefix and falls back to `index.html` for
//! client routes like `/dashboard/prime`.
//!
//! Serving is deliberately minimal and dependency-free (no `tower-http`): a small
//! traversal-safe path join plus an extension content-type map, both pure and
//! unit-tested without starting a server.

use std::path::{Path, PathBuf};

/// Env override for the dashboard bundle location, mirroring the bridge's
/// `RELIX_DASHBOARD_DIST` so an operator can point at a bundle elsewhere.
const DIST_ENV: &str = "RELUX_DASHBOARD_DIST";

/// Resolve the built dashboard bundle directory, or `None` when no bundle is
/// present (a source-only checkout that never ran the frontend build).
///
/// Resolution order: the `RELUX_DASHBOARD_DIST` override, then the committed
/// bundle relative to the current working directory (the documented path), then
/// the bundle relative to this crate's manifest so `cargo run -p relux-kernel`
/// works from anywhere in the workspace.
pub fn resolve_dist_dir() -> Option<PathBuf> {
    let has_index = |p: &Path| p.join("index.html").is_file();

    if let Ok(p) = std::env::var(DIST_ENV) {
        let pb = PathBuf::from(p);
        if has_index(&pb) {
            return Some(pb);
        }
    }
    let cwd = PathBuf::from("crates/relix-web-bridge/dashboard-dist");
    if has_index(&cwd) {
        return Some(cwd);
    }
    let manifest =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../relix-web-bridge/dashboard-dist");
    if has_index(&manifest) {
        return Some(manifest);
    }
    None
}

/// The content type for a path, keyed on its extension. Defaults to
/// `application/octet-stream` for anything unrecognized so a byte stream is
/// never mislabeled as text.
pub fn content_type_for(path: &str) -> &'static str {
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit('.').next())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Resolve a request-relative path (the part AFTER `/dashboard/`) to a real file
/// inside `dir`, or `None` when it escapes the directory or is not a file.
///
/// Traversal safety is structural: any `..` segment rejects the whole path, and
/// a backslash (a Windows separator that must never appear in a URL path) does
/// too. The result is always a file strictly inside `dir`.
pub fn resolve_asset(dir: &Path, rel: &str) -> Option<PathBuf> {
    if rel.contains('\\') {
        return None;
    }
    let mut out = dir.to_path_buf();
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return None;
        }
        out.push(seg);
    }
    if out.is_file() {
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_cover_the_vite_bundle() {
        assert_eq!(
            content_type_for("assets/index-abc.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type_for("assets/index-abc.css"),
            "text/css; charset=utf-8"
        );
        assert_eq!(content_type_for("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type_for("favicon.svg"), "image/svg+xml");
        // Unknown / extensionless never masquerades as text.
        assert_eq!(content_type_for("noext"), "application/octet-stream");
        assert_eq!(content_type_for("file.bin"), "application/octet-stream");
    }

    #[test]
    fn resolve_asset_serves_files_and_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(assets.join("app.js"), b"console.log(1)").unwrap();
        std::fs::write(dir.path().join("index.html"), b"<html></html>").unwrap();

        // A real bundle file resolves.
        assert!(resolve_asset(dir.path(), "assets/app.js").is_some());
        assert!(resolve_asset(dir.path(), "index.html").is_some());
        // Leading-slash / dot segments are normalized harmlessly.
        assert!(resolve_asset(dir.path(), "./assets/app.js").is_some());
        // A missing file is None (the caller decides 404 vs SPA fallback).
        assert!(resolve_asset(dir.path(), "assets/missing.js").is_none());
        // Traversal is refused outright, even toward a file that exists.
        std::fs::write(dir.path().join("secret.txt"), b"x").unwrap();
        assert!(resolve_asset(&assets, "../secret.txt").is_none());
        assert!(resolve_asset(dir.path(), "../Cargo.toml").is_none());
        assert!(resolve_asset(dir.path(), "..\\Cargo.toml").is_none());
    }

    #[test]
    fn committed_bundle_resolves_for_serve() {
        // The repo ships the committed bundle, so the standalone shell has
        // something to serve. (Guards against a forgotten `npm run build`.)
        assert!(
            resolve_dist_dir().is_some(),
            "expected the committed dashboard bundle to resolve for `relux-kernel serve`"
        );
    }
}
