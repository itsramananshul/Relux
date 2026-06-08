//! Local plugin manifest loader.
//!
//! Scans a directory for `*/relux-plugin.json` files, parses each into a
//! `relux_core::PluginManifest`, and validates it against the manifest contract
//! (`relux_core::validate_manifest`). This is the local-index half of the
//! Plugin Kernel Layer (`docs/RELUX_MASTER_PLAN.md` section 7.4): no registry, no
//! network - just static manifests on disk.

use std::fs;
use std::path::{Path, PathBuf};

use relux_core::plugin::validate_manifest;
use relux_core::PluginManifest;

use crate::KernelError;

/// The manifest filename every plugin directory must contain.
pub const MANIFEST_FILENAME: &str = "relux-plugin.json";

/// Scan `dir` for `*/relux-plugin.json`, parse and validate each manifest.
///
/// Directory entries are sorted before reading so the returned order is
/// deterministic regardless of the underlying filesystem's iteration order.
pub fn load_plugin_manifests(dir: &Path) -> Result<Vec<PluginManifest>, KernelError> {
    let read = fs::read_dir(dir).map_err(|e| KernelError::Io {
        path: dir.display().to_string(),
        message: e.to_string(),
    })?;

    let mut manifest_paths: Vec<PathBuf> = Vec::new();
    for entry in read {
        let entry = entry.map_err(|e| KernelError::Io {
            path: dir.display().to_string(),
            message: e.to_string(),
        })?;
        let path = entry.path();
        if path.is_dir() {
            let manifest_path = path.join(MANIFEST_FILENAME);
            if manifest_path.is_file() {
                manifest_paths.push(manifest_path);
            }
        }
    }
    manifest_paths.sort();

    let mut manifests = Vec::with_capacity(manifest_paths.len());
    for path in manifest_paths {
        let text = fs::read_to_string(&path).map_err(|e| KernelError::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        let manifest: PluginManifest =
            serde_json::from_str(&text).map_err(|e| KernelError::ManifestParse {
                path: path.display().to_string(),
                message: e.to_string(),
            })?;
        validate_manifest(&manifest).map_err(|source| KernelError::ManifestInvalid {
            path: path.display().to_string(),
            source,
        })?;
        manifests.push(manifest);
    }

    Ok(manifests)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Path to the workspace's static example plugins, resolved at compile time
    /// from this crate's manifest dir so the test is independent of the working
    /// directory `cargo test` is invoked from.
    fn examples_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/relux-plugins")
    }

    #[test]
    fn loads_both_example_manifests() {
        let manifests = load_plugin_manifests(&examples_dir()).expect("load examples");
        let ids: Vec<&str> = manifests.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["relux-adapter-local-prime", "relux-tools-echo"],
            "manifests must load in sorted, deterministic order"
        );
    }

    #[test]
    fn missing_dir_is_an_io_error() {
        let err = load_plugin_manifests(Path::new("does-not-exist-xyz")).unwrap_err();
        assert!(matches!(err, KernelError::Io { .. }), "got {err:?}");
    }
}
