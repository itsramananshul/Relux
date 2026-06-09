//! Durable plugin installation lifecycle.
//!
//! This is the backend foundation for the future Plugins tab "+" button
//! (`docs/RELUX_MASTER_PLAN.md` section 7.4 Plugin Kernel Layer, section 9.4 Plugin entity):
//! a user picks a GitHub URL or a local zip/folder, the kernel validates the
//! plugin's `relux-plugin.json`, copies/extracts it into a durable directory, and
//! records it as installed so it stays installed across restarts until removed.
//!
//! Everything here is local-only and safe by construction:
//!
//! - Manifests are validated with `relux_core::validate_manifest` before anything
//!   is registered or enabled.
//! - The install target is always `<installed_root>/<plugin-id>`; plugin ids are
//!   restricted to a safe character set so they can never escape `installed_root`.
//! - Zip extraction rejects path-traversal entries (`..`, absolute paths) - no
//!   entry may write outside the extraction directory.
//! - Removal only ever deletes a directory that lives inside `installed_root`,
//!   never an arbitrary path, and bundled plugins are refused.
//! - GitHub install shells out to `git clone --depth 1` with an argv (no shell),
//!   embeds no credentials, and reports a clear error if `git` is missing.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use relux_core::plugin::validate_manifest;
use relux_core::{InstalledPlugin, PluginId, PluginManifest, PluginSourceKind};

use crate::loader::MANIFEST_FILENAME;
use crate::{KernelError, KernelState};

/// Install a plugin from a local folder.
///
/// `source_dir` may either contain `relux-plugin.json` directly, or be a parent
/// folder containing exactly one subdirectory that has one. The located plugin
/// folder is copied into `<installed_root>/<plugin-id>`, the manifest is
/// validated and registered, and the install is recorded as enabled.
pub fn install_from_dir(
    source_dir: &Path,
    installed_root: &Path,
    kernel: &mut KernelState,
) -> Result<InstalledPlugin, KernelError> {
    let (manifest_dir, manifest) = locate_plugin_dir(source_dir)?;
    install_located(
        kernel,
        &manifest_dir,
        manifest,
        installed_root,
        PluginSourceKind::LocalDir,
        source_dir.display().to_string(),
    )
}

/// Install a plugin from a local `.zip` archive.
///
/// The archive is extracted into a staging directory under `installed_root`
/// (rejecting any path-traversal entries), the plugin folder is located the same
/// way as [`install_from_dir`], then copied into its durable install directory.
pub fn install_from_zip(
    zip_path: &Path,
    installed_root: &Path,
    kernel: &mut KernelState,
) -> Result<InstalledPlugin, KernelError> {
    fs::create_dir_all(installed_root).map_err(io_err(installed_root))?;
    let staging = installed_root.join(".staging-zip");
    if staging.exists() {
        remove_dir_within(installed_root, &staging)?;
    }

    let result = (|| {
        extract_zip(zip_path, &staging)?;
        let (manifest_dir, manifest) = locate_plugin_dir(&staging)?;
        install_located(
            kernel,
            &manifest_dir,
            manifest,
            installed_root,
            PluginSourceKind::Zip,
            zip_path.display().to_string(),
        )
    })();

    // Always clean up the staging directory, success or failure.
    let _ = remove_dir_within(installed_root, &staging);
    result
}

/// Install a plugin from a GitHub repository URL.
///
/// Pragmatic MVP: `git clone --depth 1 -- <url> <staging>` via argv (no shell),
/// then the cloned tree is treated as a local install source. `git` must be on
/// PATH; a missing binary or a failed clone returns a clear error. No
/// credentials or tokens are embedded.
pub fn install_from_github(
    url: &str,
    installed_root: &Path,
    kernel: &mut KernelState,
) -> Result<InstalledPlugin, KernelError> {
    validate_github_url(url)?;
    fs::create_dir_all(installed_root).map_err(io_err(installed_root))?;
    let staging = installed_root.join(".staging-git");
    if staging.exists() {
        remove_dir_within(installed_root, &staging)?;
    }

    let status = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--")
        .arg(url)
        .arg(&staging)
        .status()
        .map_err(|e| {
            KernelError::PluginInstall(format!("failed to run git (is it installed?): {e}"))
        });

    let result = match status {
        Err(e) => Err(e),
        Ok(s) if !s.success() => Err(KernelError::PluginInstall(format!(
            "git clone failed for {url}"
        ))),
        Ok(_) => {
            let (manifest_dir, manifest) = locate_plugin_dir(&staging)?;
            install_located(
                kernel,
                &manifest_dir,
                manifest,
                installed_root,
                PluginSourceKind::Github,
                url.to_string(),
            )
        }
    };

    let _ = remove_dir_within(installed_root, &staging);
    result
}

/// Remove an installed plugin: delete its install directory (if it lives inside
/// `installed_root`) and drop its kernel metadata + manifest registration.
///
/// Bundled plugins (the shipped Prime/echo fixtures) are refused for now because
/// the Prime demo depends on them.
pub fn remove_plugin(
    plugin_id: &str,
    installed_root: &Path,
    kernel: &mut KernelState,
) -> Result<(), KernelError> {
    let id = PluginId::new(plugin_id);
    let (source_kind, install_dir) = {
        let installed = kernel
            .installed_plugin(&id)
            .ok_or_else(|| KernelError::PluginNotInstalled(plugin_id.to_string()))?;
        (installed.source_kind.clone(), installed.install_dir.clone())
    };

    if source_kind == PluginSourceKind::Bundled {
        return Err(KernelError::BundledPluginProtected(plugin_id.to_string()));
    }

    // Only ever delete a directory that lives inside installed_root.
    let install_dir = PathBuf::from(&install_dir);
    if install_dir.exists() && dir_within(installed_root, &install_dir) {
        remove_dir_within(installed_root, &install_dir)?;
    }

    kernel.remove_installed_plugin(&id)?;
    Ok(())
}

/// List all installed plugin records (sorted by id).
pub fn list_installed(kernel: &KernelState) -> Vec<&InstalledPlugin> {
    kernel.installed_plugins()
}

// --- Internals -------------------------------------------------------------

/// Copy a located plugin folder into its durable install directory, then
/// register + record it. Shared by every install path.
///
/// If the id is already installed as [`PluginSourceKind::Bundled`] and the new
/// source is not bundled, the existing bundled record is kept untouched (so a
/// stray `install-dir examples/...` cannot silently convert a protected bundled
/// plugin into a removable one); the call is otherwise idempotent and cleanly
/// replaces any prior record for the same id.
fn install_located(
    kernel: &mut KernelState,
    manifest_dir: &Path,
    manifest: PluginManifest,
    installed_root: &Path,
    source_kind: PluginSourceKind,
    source_label: String,
) -> Result<InstalledPlugin, KernelError> {
    let id = manifest.id.clone();
    safe_plugin_id(id.as_str())?;

    if let Some(existing) = kernel.installed_plugin(&id) {
        if existing.source_kind == PluginSourceKind::Bundled
            && source_kind != PluginSourceKind::Bundled
        {
            return Ok(existing.clone());
        }
    }

    let target = installed_root.join(id.as_str());
    ensure_within(installed_root, &target)?;
    if target.exists() {
        remove_dir_within(installed_root, &target)?;
    }
    fs::create_dir_all(&target).map_err(io_err(&target))?;
    copy_dir_recursive(manifest_dir, &target)?;

    Ok(kernel.install_plugin(
        manifest,
        source_kind,
        source_label,
        target.display().to_string(),
        true,
    ))
}

/// Find the plugin folder (and parsed, validated manifest) inside `dir`.
///
/// Accepts either a folder that directly contains `relux-plugin.json`, or a
/// parent folder containing exactly one subdirectory that does.
fn locate_plugin_dir(dir: &Path) -> Result<(PathBuf, PluginManifest), KernelError> {
    let direct = dir.join(MANIFEST_FILENAME);
    if direct.is_file() {
        let manifest = read_manifest(&direct)?;
        return Ok((dir.to_path_buf(), manifest));
    }

    let read = fs::read_dir(dir).map_err(io_err(dir))?;
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in read {
        let entry = entry.map_err(io_err(dir))?;
        let path = entry.path();
        if path.is_dir() && path.join(MANIFEST_FILENAME).is_file() {
            candidates.push(path);
        }
    }
    candidates.sort();

    match candidates.len() {
        1 => {
            let plugin_dir = candidates.remove(0);
            let manifest = read_manifest(&plugin_dir.join(MANIFEST_FILENAME))?;
            Ok((plugin_dir, manifest))
        }
        0 => Err(KernelError::PluginInstall(format!(
            "no {MANIFEST_FILENAME} found in {}",
            dir.display()
        ))),
        n => Err(KernelError::PluginInstall(format!(
            "found {n} plugin folders in {}; expected exactly one",
            dir.display()
        ))),
    }
}

/// Read, parse, and validate a `relux-plugin.json` manifest.
fn read_manifest(manifest_path: &Path) -> Result<PluginManifest, KernelError> {
    let text = fs::read_to_string(manifest_path).map_err(io_err(manifest_path))?;
    let manifest: PluginManifest =
        serde_json::from_str(&text).map_err(|e| KernelError::ManifestParse {
            path: manifest_path.display().to_string(),
            message: e.to_string(),
        })?;
    validate_manifest(&manifest).map_err(|source| KernelError::ManifestInvalid {
        path: manifest_path.display().to_string(),
        source,
    })?;
    Ok(manifest)
}

/// Reject plugin ids that could escape the install root or contain path
/// separators. Ids are expected to look like `relux-tools-echo`.
fn safe_plugin_id(id: &str) -> Result<(), KernelError> {
    let ok = !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !id.contains("..");
    if ok {
        Ok(())
    } else {
        Err(KernelError::UnsafePluginPath(id.to_string()))
    }
}

/// Validate that a GitHub URL is well-formed and credential-free.
fn validate_github_url(url: &str) -> Result<(), KernelError> {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed != url {
        return Err(KernelError::PluginInstall(format!(
            "invalid GitHub URL: {url:?}"
        )));
    }
    let is_github = trimmed.starts_with("https://github.com/")
        || trimmed.starts_with("https://www.github.com/");
    if !is_github {
        return Err(KernelError::PluginInstall(format!(
            "unsupported GitHub URL (expected https://github.com/...): {url}"
        )));
    }
    // Reject embedded userinfo / credentials like https://user:tok@github.com/.
    if trimmed.contains('@') {
        return Err(KernelError::PluginInstall(
            "credentials embedded in URL are not allowed".to_string(),
        ));
    }
    Ok(())
}

/// Extract a zip into `dest`, rejecting any entry that would escape `dest`.
fn extract_zip(zip_path: &Path, dest: &Path) -> Result<(), KernelError> {
    let file = fs::File::open(zip_path).map_err(io_err(zip_path))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        KernelError::PluginInstall(format!("open zip {}: {e}", zip_path.display()))
    })?;
    fs::create_dir_all(dest).map_err(io_err(dest))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| KernelError::PluginInstall(format!("read zip entry {i}: {e}")))?;

        // `enclosed_name` returns None for any traversal/absolute entry; treat
        // that as a hard rejection rather than silently skipping it.
        let raw_name = entry.name().to_string();
        let enclosed = entry
            .enclosed_name()
            .ok_or_else(|| KernelError::UnsafePluginPath(raw_name.clone()))?
            .to_path_buf();

        let out_path = dest.join(&enclosed);
        ensure_within(dest, &out_path)?;

        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(io_err(&out_path))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| KernelError::PluginInstall(format!("read zip entry {raw_name}: {e}")))?;
        fs::write(&out_path, &bytes).map_err(io_err(&out_path))?;
    }
    Ok(())
}

/// Copy a directory tree from `src` into `dst`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), KernelError> {
    fs::create_dir_all(dst).map_err(io_err(dst))?;
    for entry in fs::read_dir(src).map_err(io_err(src))? {
        let entry = entry.map_err(io_err(src))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(io_err(&to))?;
        }
    }
    Ok(())
}

/// Lexically normalize a path (resolve `.`/`..` without touching the
/// filesystem) so containment can be checked for paths that may not exist yet.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True if `candidate` is `root` or lives inside it (lexically).
fn dir_within(root: &Path, candidate: &Path) -> bool {
    normalize_lexical(candidate).starts_with(normalize_lexical(root))
}

fn ensure_within(root: &Path, candidate: &Path) -> Result<(), KernelError> {
    if dir_within(root, candidate) {
        Ok(())
    } else {
        Err(KernelError::UnsafePluginPath(candidate.display().to_string()))
    }
}

/// Remove a directory, but only if it lives strictly inside `root` (never the
/// root itself and never an arbitrary outside path).
fn remove_dir_within(root: &Path, target: &Path) -> Result<(), KernelError> {
    ensure_within(root, target)?;
    if normalize_lexical(target) == normalize_lexical(root) {
        return Err(KernelError::UnsafePluginPath(target.display().to_string()));
    }
    fs::remove_dir_all(target).map_err(io_err(target))
}

/// Build a closure that maps an io error to [`KernelError::Io`] for `path`.
fn io_err(path: &Path) -> impl Fn(std::io::Error) -> KernelError + '_ {
    move |e| KernelError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use relux_core::PluginId;

    use crate::SqliteStore;

    /// A valid ToolSet `relux-plugin.json` body for a test plugin id.
    fn manifest_json(id: &str) -> String {
        format!(
            "{{\
\"id\":\"{id}\",\
\"name\":\"Test {id}\",\
\"version\":\"0.1.0\",\
\"kind\":\"ToolSet\",\
\"description\":\"test plugin\",\
\"author\":\"test\",\
\"trust_level\":\"private\",\
\"capabilities\":{{\"tools\":[],\"permissions\":[\"tool:{id}:noop\"]}},\
\"health\":\"unknown\"\
}}"
        )
    }

    /// Write a plugin folder containing `relux-plugin.json` at `dir`.
    fn write_plugin_dir(dir: &Path, id: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(MANIFEST_FILENAME), manifest_json(id)).unwrap();
    }

    /// A kernel with one plugin registered as Bundled, mirroring bootstrap.
    fn kernel_with_bundled_echo() -> KernelState {
        let mut kernel = KernelState::new();
        let manifest: PluginManifest =
            serde_json::from_str(&manifest_json("relux-tools-echo")).unwrap();
        kernel.install_plugin(
            manifest,
            PluginSourceKind::Bundled,
            "bundled example".to_string(),
            "examples/relux-plugins/relux-tools-echo".to_string(),
            true,
        );
        kernel
    }

    #[test]
    fn install_from_dir_registers_and_persists_across_store_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("src").join("relux-tools-demo");
        write_plugin_dir(&source, "relux-tools-demo");
        let installed_root = tmp.path().join("installed");

        let mut kernel = KernelState::new();
        let installed =
            install_from_dir(&source, &installed_root, &mut kernel).expect("install ok");

        // Manifest registered + metadata recorded.
        let id = PluginId::new("relux-tools-demo");
        assert!(kernel.plugin(&id).is_some(), "manifest registered");
        assert_eq!(kernel.installed_plugin_count(), 1);
        assert_eq!(installed.source_kind, PluginSourceKind::LocalDir);
        assert!(installed.enabled);
        // Files were copied into the durable install dir.
        assert!(installed_root
            .join("relux-tools-demo")
            .join(MANIFEST_FILENAME)
            .is_file());

        // Persist through the SqliteStore and reload: install survives.
        let db = tmp.path().join("local.db");
        {
            let mut store = SqliteStore::open(&db).unwrap();
            store.save(&kernel).unwrap();
        }
        let store = SqliteStore::open(&db).unwrap();
        let reloaded = store.load().unwrap();
        assert_eq!(reloaded.installed_plugin_count(), 1);
        let record = reloaded.installed_plugin(&id).expect("survives reload");
        assert_eq!(record.version, "0.1.0");
        assert_eq!(record.source_kind, PluginSourceKind::LocalDir);
        assert!(reloaded.plugin(&id).is_some(), "manifest survives reload");
    }

    #[test]
    fn install_from_parent_dir_with_single_plugin_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("clone");
        write_plugin_dir(&parent.join("relux-tools-demo"), "relux-tools-demo");
        let installed_root = tmp.path().join("installed");

        let mut kernel = KernelState::new();
        let installed = install_from_dir(&parent, &installed_root, &mut kernel).expect("install ok");
        assert_eq!(installed.id, PluginId::new("relux-tools-demo"));
    }

    #[test]
    fn repeated_install_is_idempotent_and_does_not_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("relux-tools-demo");
        write_plugin_dir(&source, "relux-tools-demo");
        let installed_root = tmp.path().join("installed");

        let mut kernel = KernelState::new();
        install_from_dir(&source, &installed_root, &mut kernel).unwrap();
        install_from_dir(&source, &installed_root, &mut kernel).unwrap();
        assert_eq!(
            kernel.installed_plugin_count(),
            1,
            "re-install must replace, not duplicate"
        );
        assert_eq!(kernel.plugin_count(), 1);
    }

    #[test]
    fn remove_non_bundled_clears_metadata_and_dir_but_refuses_bundled() {
        let tmp = tempfile::tempdir().unwrap();
        let installed_root = tmp.path().join("installed");

        // Start from a bundled-echo kernel (cannot be removed) and add a
        // user-installed plugin (can be removed).
        let mut kernel = kernel_with_bundled_echo();
        let source = tmp.path().join("relux-tools-demo");
        write_plugin_dir(&source, "relux-tools-demo");
        install_from_dir(&source, &installed_root, &mut kernel).unwrap();
        assert_eq!(kernel.installed_plugin_count(), 2);

        // Bundled echo is protected.
        let err = remove_plugin("relux-tools-echo", &installed_root, &mut kernel).unwrap_err();
        assert!(
            matches!(err, KernelError::BundledPluginProtected(_)),
            "got {err:?}"
        );
        assert_eq!(kernel.installed_plugin_count(), 2, "nothing removed");

        // The user plugin removes cleanly: metadata, manifest, and dir all gone.
        remove_plugin("relux-tools-demo", &installed_root, &mut kernel).unwrap();
        let id = PluginId::new("relux-tools-demo");
        assert!(kernel.installed_plugin(&id).is_none());
        assert!(kernel.plugin(&id).is_none(), "manifest unregistered");
        assert!(!installed_root.join("relux-tools-demo").exists(), "dir gone");
        assert_eq!(kernel.installed_plugin_count(), 1);

        // Removing something that is not installed is a clear error.
        let err = remove_plugin("relux-tools-nope", &installed_root, &mut kernel).unwrap_err();
        assert!(matches!(err, KernelError::PluginNotInstalled(_)), "got {err:?}");
    }

    #[test]
    fn zip_install_installs_safe_fixture_and_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let installed_root = tmp.path().join("installed");

        // 1. A safe zip with one plugin folder installs.
        let safe_zip = tmp.path().join("safe.zip");
        {
            let file = fs::File::create(&safe_zip).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zw.start_file("relux-tools-zip/relux-plugin.json", opts).unwrap();
            zw.write_all(manifest_json("relux-tools-zip").as_bytes())
                .unwrap();
            zw.finish().unwrap();
        }
        let mut kernel = KernelState::new();
        let installed = install_from_zip(&safe_zip, &installed_root, &mut kernel).expect("zip ok");
        assert_eq!(installed.source_kind, PluginSourceKind::Zip);
        assert!(kernel.plugin(&PluginId::new("relux-tools-zip")).is_some());
        assert!(installed_root
            .join("relux-tools-zip")
            .join(MANIFEST_FILENAME)
            .is_file());
        // The staging directory was cleaned up.
        assert!(!installed_root.join(".staging-zip").exists());

        // 2. A zip with a traversal entry is rejected and installs nothing.
        let evil_zip = tmp.path().join("evil.zip");
        {
            let file = fs::File::create(&evil_zip).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zw.start_file("../escape.txt", opts).unwrap();
            zw.write_all(b"pwned").unwrap();
            zw.finish().unwrap();
        }
        let mut kernel2 = KernelState::new();
        let err = install_from_zip(&evil_zip, &installed_root, &mut kernel2).unwrap_err();
        assert!(matches!(err, KernelError::UnsafePluginPath(_)), "got {err:?}");
        assert_eq!(kernel2.installed_plugin_count(), 0);
        // No file escaped the install root.
        assert!(!tmp.path().join("escape.txt").exists());
    }

    #[test]
    fn github_url_validation_rejects_bad_and_credentialed_urls() {
        assert!(validate_github_url("https://github.com/owner/repo").is_ok());
        assert!(validate_github_url("https://example.com/owner/repo").is_err());
        assert!(validate_github_url("ftp://github.com/owner/repo").is_err());
        assert!(validate_github_url("https://tok@github.com/owner/repo").is_err());
        assert!(validate_github_url(" https://github.com/owner/repo ").is_err());
    }

    #[test]
    fn unsafe_plugin_ids_are_rejected() {
        assert!(safe_plugin_id("relux-tools-echo").is_ok());
        assert!(safe_plugin_id("..").is_err());
        assert!(safe_plugin_id("a/b").is_err());
        assert!(safe_plugin_id("a\\b").is_err());
        assert!(safe_plugin_id("..evil").is_err());
        assert!(safe_plugin_id("").is_err());
    }
}
