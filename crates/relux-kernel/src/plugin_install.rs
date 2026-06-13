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

use relux_core::permission::ToolDefinition;
use relux_core::plugin::validate_manifest;
use relux_core::{
    InstalledPlugin, PluginCapability, PluginHealth, PluginId, PluginKind, PluginManifest,
    PluginSourceKind, TrustLevel,
};

/// The `author` sentinel stamped on a manifest Relux generated itself because a
/// downloaded/imported source carried no `relux-plugin.json`. It is the honest
/// marker the kernel/dashboard use to say "installed as metadata only" - the
/// plugin ships no runnable tools until the operator configures a runtime or adds
/// tool definitions. See [`scaffold_manifest`] and [`is_generated_manifest`].
pub const GENERATED_MANIFEST_AUTHOR: &str = "relux (generated manifest)";

/// Whether `manifest` was scaffolded by Relux (no `relux-plugin.json` was present
/// in the source). A generated manifest declares no tools and is non-executable
/// until the operator configures it.
pub fn is_generated_manifest(manifest: &PluginManifest) -> bool {
    manifest.author == GENERATED_MANIFEST_AUTHOR
}

use crate::loader::{load_plugin_manifests, MANIFEST_FILENAME};
use crate::state::{BundledRefresh, BundledRefreshSummary};
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
    let (manifest_dir, manifest) = locate_or_scaffold(source_dir, &seed_from_path(source_dir))?;
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
        let zip_seed = zip_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("plugin")
            .to_string();
        let (manifest_dir, manifest) = locate_or_scaffold(&staging, &zip_seed)?;
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
            let (manifest_dir, manifest) = locate_or_scaffold(&staging, &github_repo_seed(url))?;
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

/// Idempotently refresh every shipped bundled plugin manifest under
/// `examples_dir` into `kernel` (`docs/RELUX_MASTER_PLAN.md` section 9.4, section
/// 7.4).
///
/// This is the central, restart-safe bootstrap seam: it loads the bundled
/// manifests from disk and reconciles each against the live control plane via
/// [`KernelState::refresh_bundled_plugin`]. It is safe to call on EVERY load -
/// fresh store or long-lived one - so an existing local DB picks up newly shipped
/// adapters/tools without a reset, while never duplicating records, downgrading
/// the protected `Bundled` source, overwriting a user-installed plugin, or
/// touching per-plugin runtime config / local user state.
///
/// The `install_dir`/`source_label` recorded for each bundled plugin match the
/// original bootstrap (`<examples_dir>/<plugin-id>`, `"bundled example"`) so an
/// already-current record is recognized as unchanged and emits no audit noise.
pub fn refresh_bundled_plugins(
    kernel: &mut KernelState,
    examples_dir: &Path,
) -> Result<BundledRefreshSummary, KernelError> {
    let manifests = load_plugin_manifests(examples_dir)?;
    let mut summary = BundledRefreshSummary::default();
    for manifest in manifests {
        let id = manifest.id.as_str().to_string();
        let install_dir = examples_dir.join(&id).display().to_string();
        match kernel.refresh_bundled_plugin(manifest, "bundled example".to_string(), install_dir) {
            BundledRefresh::Added => summary.added.push(id),
            BundledRefresh::Updated => summary.updated.push(id),
            BundledRefresh::Unchanged => summary.unchanged += 1,
            BundledRefresh::SkippedUserInstalled => summary.skipped_user_installed.push(id),
        }
    }
    Ok(summary)
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

/// Locate the plugin folder + validated manifest inside `dir`, or scaffold a
/// safe wrapper manifest when the source carries no `relux-plugin.json`.
///
/// This is what lets an arbitrary GitHub repo / local folder / zip be installed
/// even with no Relux manifest (`docs/RELUX_MASTER_PLAN.md` section 7.4): the
/// generated manifest is **metadata only** - it declares NO tools and is
/// non-executable until the operator configures a loopback runtime or adds tool
/// definitions. Relux never infers tool commands from repo content. An ambiguous
/// source (more than one real plugin folder) is still a hard error rather than a
/// silent guess.
fn locate_or_scaffold(
    dir: &Path,
    id_seed: &str,
) -> Result<(PathBuf, PluginManifest), KernelError> {
    match try_locate_plugin_dir(dir)? {
        Some(found) => Ok(found),
        None => Ok((dir.to_path_buf(), scaffold_manifest(id_seed, dir)?)),
    }
}

/// Find the plugin folder (and parsed, validated manifest) inside `dir`, or
/// `Ok(None)` when no `relux-plugin.json` is present anywhere obvious.
///
/// Accepts either a folder that directly contains `relux-plugin.json`, or a
/// parent folder containing exactly one subdirectory that does. More than one
/// candidate is an ambiguous source and a hard error (never a silent guess).
fn try_locate_plugin_dir(dir: &Path) -> Result<Option<(PathBuf, PluginManifest)>, KernelError> {
    let direct = dir.join(MANIFEST_FILENAME);
    if direct.is_file() {
        let manifest = read_manifest(&direct)?;
        return Ok(Some((dir.to_path_buf(), manifest)));
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
            Ok(Some((plugin_dir, manifest)))
        }
        0 => Ok(None),
        n => Err(KernelError::PluginInstall(format!(
            "found {n} plugin folders in {}; expected exactly one",
            dir.display()
        ))),
    }
}

/// Build a safe, non-executable wrapper [`PluginManifest`] for a source that has
/// no `relux-plugin.json`.
///
/// The id is derived from `id_seed` (repo/folder/zip name) and sanitized so it
/// can never escape the install root or collide with a bundled id
/// (`relux-plugin-<seed>`). The manifest declares NO tools and NO permissions, is
/// stamped [`TrustLevel::Unverified`] and authored by [`GENERATED_MANIFEST_AUTHOR`]
/// so it reads honestly as "metadata only". A README first line, when present,
/// becomes the description.
fn scaffold_manifest(id_seed: &str, dir: &Path) -> Result<PluginManifest, KernelError> {
    let sanitized = sanitize_seed(id_seed);
    let id = format!("relux-plugin-{sanitized}");
    let summary = read_readme_summary(dir);
    let description = match summary {
        Some(s) => format!(
            "{s} (Installed as metadata: no runnable tools yet - configure a runtime or add tool definitions before it can run.)"
        ),
        None => "Installed as metadata: no runnable tools yet - configure a runtime or add tool definitions before it can run.".to_string(),
    };
    let manifest = PluginManifest {
        id: PluginId::new(&id),
        name: format!("{sanitized} (metadata only)"),
        version: "0.0.0".to_string(),
        kind: PluginKind::ToolSet,
        description,
        author: GENERATED_MANIFEST_AUTHOR.to_string(),
        trust_level: TrustLevel::Unverified,
        capabilities: PluginCapability {
            tools: Vec::<ToolDefinition>::new(),
            permissions: Vec::new(),
        },
        health: PluginHealth::Unknown,
    };
    // Validate the generated manifest the same way a real one is validated, so a
    // scaffolded record can never be subtly malformed (e.g. an empty id/name).
    validate_manifest(&manifest).map_err(|source| KernelError::ManifestInvalid {
        path: dir.display().to_string(),
        source,
    })?;
    Ok(manifest)
}

/// The first non-empty line of a README in `dir` (bounded), or `None`. Used only
/// to give a scaffolded manifest a human-readable description.
fn read_readme_summary(dir: &Path) -> Option<String> {
    for name in ["README.md", "README", "README.txt", "readme.md", "Readme.md"] {
        let p = dir.join(name);
        if p.is_file() {
            if let Ok(text) = fs::read_to_string(&p) {
                for line in text.lines() {
                    let l = line.trim().trim_start_matches('#').trim();
                    if !l.is_empty() {
                        return Some(l.chars().take(200).collect());
                    }
                }
            }
        }
    }
    None
}

/// The last path component of `path`, used as a plugin-id seed.
fn seed_from_path(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("plugin")
        .to_string()
}

/// Derive a plugin-id seed from a GitHub URL: the trailing repo name without a
/// `.git` suffix.
pub(crate) fn github_repo_seed(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or("plugin");
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

/// Reduce a seed to a safe id fragment: lowercase ASCII alphanumerics, with any
/// other character collapsed to a single `-`. Leading/trailing dashes are
/// trimmed. The result never contains `..`, a path separator, or a `.`, so
/// `relux-plugin-<seed>` always passes [`safe_plugin_id`]. An empty result
/// degrades to `"plugin"`.
pub(crate) fn sanitize_seed(seed: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in seed.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "plugin".to_string()
    } else {
        trimmed.to_string()
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

    use relux_core::namespace::NamespaceKind;
    use relux_core::{AgentId, NamespaceId, PluginId};

    use crate::SqliteStore;

    /// The workspace's shipped example plugins, resolved from this crate's
    /// manifest dir so the test is independent of the invoking working directory.
    fn examples_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/relux-plugins")
    }

    /// The five plugin ids shipped under `examples/relux-plugins`, sorted.
    const BUNDLED_IDS: &[&str] = &[
        "relux-adapter-claude-cli",
        "relux-adapter-codex-cli",
        "relux-adapter-local-prime",
        "relux-tools-echo",
        "relux-tools-status",
    ];

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
    fn install_dir_without_manifest_generates_safe_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        // A repo-like folder with NO relux-plugin.json, but a README.
        let source = tmp.path().join("my-cool-repo");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("README.md"), "# My Cool Repo\n\nDoes cool things.\n").unwrap();
        fs::write(source.join("main.py"), "print('hi')\n").unwrap();
        let installed_root = tmp.path().join("installed");

        let mut kernel = KernelState::new();
        let installed = install_from_dir(&source, &installed_root, &mut kernel).expect("install ok");

        // A safe, derived id; non-executable (no tools); marked generated.
        assert_eq!(installed.id, PluginId::new("relux-plugin-my-cool-repo"));
        assert_eq!(installed.version, "0.0.0");
        assert_eq!(installed.source_kind, PluginSourceKind::LocalDir);
        let manifest = kernel.plugin(&installed.id).expect("manifest registered");
        assert!(is_generated_manifest(manifest), "manifest marked generated");
        assert!(manifest.capabilities.tools.is_empty(), "no tools => non-executable");
        assert!(manifest.capabilities.permissions.is_empty());
        assert_eq!(manifest.trust_level, TrustLevel::Unverified);
        assert!(manifest.description.contains("My Cool Repo"), "README summary used");
        // No tool is runnable from a generated manifest.
        let tools = kernel.discover_tools(None);
        assert!(
            !tools.iter().any(|t| t.plugin_id == installed.id.as_str()),
            "generated plugin exposes no runnable tools"
        );

        // A real manifest in the same folder is still preferred over scaffolding.
        let source2 = tmp.path().join("real");
        write_plugin_dir(&source2, "relux-tools-real");
        let installed2 = install_from_dir(&source2, &installed_root, &mut kernel).expect("ok");
        assert_eq!(installed2.id, PluginId::new("relux-tools-real"));
        assert!(!is_generated_manifest(kernel.plugin(&installed2.id).unwrap()));
    }

    #[test]
    fn scaffold_sanitizes_malicious_seeds() {
        // Path-traversal / separator / weird seeds all reduce to a safe id that
        // passes safe_plugin_id and cannot escape the install root.
        for (seed, expect) in [
            ("../../etc/passwd", "relux-plugin-etc-passwd"),
            ("..", "relux-plugin-plugin"),
            ("a/b\\c", "relux-plugin-a-b-c"),
            ("My Repo.git", "relux-plugin-my-repo-git"),
            ("   ", "relux-plugin-plugin"),
            ("UPPER_case", "relux-plugin-upper-case"),
        ] {
            let id = format!("relux-plugin-{}", sanitize_seed(seed));
            assert_eq!(id, expect, "seed {seed:?}");
            assert!(safe_plugin_id(&id).is_ok(), "{id} must be a safe id");
        }
    }

    #[test]
    fn github_repo_seed_strips_git_suffix_and_trailing_slash() {
        assert_eq!(github_repo_seed("https://github.com/owner/repo"), "repo");
        assert_eq!(github_repo_seed("https://github.com/owner/repo.git"), "repo");
        assert_eq!(github_repo_seed("https://github.com/owner/repo/"), "repo");
    }

    #[test]
    fn zip_without_manifest_generates_metadata_and_is_safe() {
        let tmp = tempfile::tempdir().unwrap();
        let installed_root = tmp.path().join("installed");
        // A zip with files but no relux-plugin.json anywhere.
        let zip_path = tmp.path().join("toolbox.zip");
        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zw.start_file("src/lib.rs", opts).unwrap();
            zw.write_all(b"// code").unwrap();
            zw.finish().unwrap();
        }
        let mut kernel = KernelState::new();
        let installed = install_from_zip(&zip_path, &installed_root, &mut kernel).expect("zip ok");
        assert_eq!(installed.id, PluginId::new("relux-plugin-toolbox"));
        assert!(is_generated_manifest(kernel.plugin(&installed.id).unwrap()));
        assert!(!installed_root.join(".staging-zip").exists(), "staging cleaned");
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

    /// Record a plugin as installed with the given source, mirroring a store row.
    fn install_recorded(
        kernel: &mut KernelState,
        id: &str,
        source_kind: PluginSourceKind,
        source_label: &str,
        install_dir: &str,
        enabled: bool,
    ) {
        let manifest: PluginManifest = serde_json::from_str(&manifest_json(id)).unwrap();
        kernel.install_plugin(
            manifest,
            source_kind,
            source_label.to_string(),
            install_dir.to_string(),
            enabled,
        );
    }

    /// An OLDER store (Prime present, only the original three bundled plugins
    /// recorded) gains the newly shipped bundled plugins on refresh - protected,
    /// persisted, without duplicating records or dropping Prime.
    #[test]
    fn older_store_with_prime_gains_new_bundled_plugins_on_refresh() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("local.db");

        // 1. Simulate the older store and persist it.
        {
            let mut kernel = KernelState::new();
            for id in [
                "relux-tools-echo",
                "relux-tools-status",
                "relux-adapter-local-prime",
            ] {
                install_recorded(
                    &mut kernel,
                    id,
                    PluginSourceKind::Bundled,
                    "bundled example",
                    &format!("examples/relux-plugins/{id}"),
                    true,
                );
            }
            kernel.create_namespace("workspace", "Workspace", NamespaceKind::Personal);
            kernel
                .create_agent(
                    "prime",
                    "Prime",
                    "operator",
                    &PluginId::new("relux-adapter-local-prime"),
                    &NamespaceId::new("workspace"),
                    None,
                    vec![],
                )
                .unwrap();
            let mut store = SqliteStore::open(&db).unwrap();
            store.save(&kernel).unwrap();
        }

        // 2. Boot against the older store: the CLI adapters are missing.
        let mut kernel = SqliteStore::open(&db).unwrap().load().unwrap();
        assert!(
            kernel
                .installed_plugin(&PluginId::new("relux-adapter-claude-cli"))
                .is_none(),
            "older store is missing the new CLI adapter before refresh"
        );

        let summary = refresh_bundled_plugins(&mut kernel, &examples_dir()).unwrap();
        SqliteStore::open(&db).unwrap().save(&kernel).unwrap();

        // 3. Reload and assert the new bundled plugins are present + protected,
        //    Prime survived, and there are no duplicates.
        let mut kernel = SqliteStore::open(&db).unwrap().load().unwrap();
        for id in ["relux-adapter-claude-cli", "relux-adapter-codex-cli"] {
            let rec = kernel
                .installed_plugin(&PluginId::new(id))
                .expect("new bundled plugin present after refresh");
            assert_eq!(rec.source_kind, PluginSourceKind::Bundled);
            assert!(summary.added.contains(&id.to_string()));
        }
        let ids: Vec<String> = kernel
            .installed_plugins()
            .iter()
            .map(|p| p.id.as_str().to_string())
            .collect();
        assert_eq!(ids, BUNDLED_IDS, "exactly the five shipped bundled plugins");
        assert!(
            kernel.agent(&AgentId::new("prime")).is_some(),
            "Prime survived the refresh"
        );

        // The newly added bundled plugin is non-removable.
        let err = remove_plugin("relux-adapter-claude-cli", tmp.path(), &mut kernel).unwrap_err();
        assert!(
            matches!(err, KernelError::BundledPluginProtected(_)),
            "got {err:?}"
        );
    }

    /// Refresh against an already-current store is a pure no-op: nothing added or
    /// updated, every bundled plugin unchanged, and no duplicate records.
    #[test]
    fn refresh_is_idempotent_on_a_current_store() {
        let mut kernel = KernelState::new();

        let first = refresh_bundled_plugins(&mut kernel, &examples_dir()).unwrap();
        assert_eq!(first.added.len(), BUNDLED_IDS.len(), "fresh store installs all");
        assert_eq!(kernel.installed_plugin_count(), BUNDLED_IDS.len());

        let second = refresh_bundled_plugins(&mut kernel, &examples_dir()).unwrap();
        assert!(second.added.is_empty());
        assert!(second.updated.is_empty());
        assert_eq!(second.unchanged, BUNDLED_IDS.len());
        assert!(!second.changed(), "a current store needs no save");
        assert_eq!(
            kernel.installed_plugin_count(),
            BUNDLED_IDS.len(),
            "still no duplicate records"
        );
    }

    /// A stale bundled manifest is updated in place (not duplicated) and the
    /// operator's `enabled` choice is preserved across the update.
    #[test]
    fn refresh_updates_changed_bundled_manifest_in_place() {
        let mut kernel = KernelState::new();
        // A stale, operator-disabled bundled echo at the install dir refresh uses.
        let install_dir = examples_dir()
            .join("relux-tools-echo")
            .display()
            .to_string();
        install_recorded(
            &mut kernel,
            "relux-tools-echo",
            PluginSourceKind::Bundled,
            "bundled example",
            &install_dir,
            false,
        );

        let summary = refresh_bundled_plugins(&mut kernel, &examples_dir()).unwrap();

        assert!(
            summary.updated.contains(&"relux-tools-echo".to_string()),
            "the stale manifest was updated, got {summary:?}"
        );
        let echoes = kernel
            .installed_plugins()
            .iter()
            .filter(|p| p.id.as_str() == "relux-tools-echo")
            .count();
        assert_eq!(echoes, 1, "update in place, not a duplicate");

        // The shipped manifest replaced the stale one.
        let shipped = load_plugin_manifests(&examples_dir())
            .unwrap()
            .into_iter()
            .find(|m| m.id.as_str() == "relux-tools-echo")
            .unwrap();
        let stored = kernel.plugin(&PluginId::new("relux-tools-echo")).unwrap();
        assert_eq!(stored.name, shipped.name, "manifest was refreshed");

        // The operator's enabled=false choice survives the update.
        assert!(
            !kernel
                .installed_plugin(&PluginId::new("relux-tools-echo"))
                .unwrap()
                .enabled,
            "enabled choice preserved across refresh"
        );
    }

    /// A user-installed plugin that happens to share a bundled id is NEVER
    /// overwritten by the refresh; the other bundled plugins still install.
    #[test]
    fn refresh_does_not_overwrite_user_installed_plugin() {
        let mut kernel = KernelState::new();
        install_recorded(
            &mut kernel,
            "relux-tools-status",
            PluginSourceKind::LocalDir,
            "/home/me/my-status",
            "installed/relux-tools-status",
            true,
        );

        let summary = refresh_bundled_plugins(&mut kernel, &examples_dir()).unwrap();

        let rec = kernel
            .installed_plugin(&PluginId::new("relux-tools-status"))
            .unwrap();
        assert_eq!(rec.source_kind, PluginSourceKind::LocalDir, "left as user install");
        assert_eq!(rec.source_label, "/home/me/my-status", "user metadata untouched");
        assert!(summary
            .skipped_user_installed
            .contains(&"relux-tools-status".to_string()));
        // The genuinely-bundled plugins still get installed.
        assert!(kernel
            .installed_plugin(&PluginId::new("relux-tools-echo"))
            .is_some());
    }
}
