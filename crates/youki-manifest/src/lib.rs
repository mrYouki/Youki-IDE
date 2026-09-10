//! Reads and validates a Youki Shell plugin's `plugin.json`.
//!
//! Full field reference lives in Youki Shell's own `PLUGIN_GUIDE.md`.
//! This crate only models the subset the build engine actually reads
//! before/during a build (pluginId, entry.dexPath, entry.mainClass).
//! Anything else in plugin.json passes through untouched — we don't
//! want the build tool to become a second source of truth for a
//! schema owned by Youki Shell.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEntry {
    #[serde(rename = "type")]
    pub entry_type: String,

    /// Path of the dex file inside the final plugin.zip, relative to
    /// the archive root. Does NOT have to be "classes.dex" — d8/r8
    /// always name their own output that, but the packaging stage
    /// renames it to match whatever this field says.
    #[serde(rename = "dexPath", default = "default_dex_path")]
    pub dex_path: String,

    #[serde(rename = "mainClass", default)]
    pub main_class: Option<String>,
}

fn default_dex_path() -> String {
    "classes.dex".to_string()
}

/// New, additive field: `plugin.json` can now declare the SDK/NDK
/// versions it needs. Entirely optional — a plugin.json with no
/// "requirements" object still loads fine (all fields default), so
/// this doesn't break any project written before this field existed.
///
/// `minSdk`/`targetSdk` are separate from `compileSdk` on purpose:
/// `compileSdk` picks which android.jar the compiler builds against
/// (should normally be the newest available, since a newer android.jar
/// is a superset — it doesn't force the plugin to require that newer
/// OS version). `minSdk` is the actual floor of devices the plugin
/// will run on; `targetSdk` signals which OS version's behavior the
/// plugin was written/tested against. Getting `compileSdk` and
/// `minSdk` confused is the single most common Android build
/// misconfiguration, so the two are modeled as distinct required-ish
/// fields rather than one number doing double duty.
///
/// Example in plugin.json, for a plugin supporting Android 8 (API 26)
/// through Android 15 (API 35), on every device architecture:
/// ```json
/// "requirements": {
///   "compileSdk": 35,
///   "minSdk": 26,
///   "targetSdk": 35,
///   "buildToolsVersion": "35.0.0",
///   "ndkVersion": "27.2.12479018",
///   "abis": ["arm64-v8a", "armeabi-v7a", "x86_64"]
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRequirements {
    /// android.jar version the compiler builds against. Should be the
    /// highest SDK the build machine has installed — this does NOT
    /// raise the plugin's minimum supported OS version.
    #[serde(rename = "compileSdk", default = "default_compile_sdk")]
    pub compile_sdk: u32,

    /// Oldest Android version (API level) the plugin must still run
    /// on. d8/r8's `--min-api` flag is driven directly by this value,
    /// not by compileSdk.
    #[serde(rename = "minSdk", default = "default_min_sdk")]
    pub min_sdk: u32,

    /// OS version the plugin is designed/tested for. Purely
    /// informational for this build tool today (Youki Shell itself
    /// may use it at load time); defaults to compileSdk when absent.
    #[serde(rename = "targetSdk", default)]
    pub target_sdk: Option<u32>,

    #[serde(rename = "buildToolsVersion", default = "default_build_tools_version")]
    pub build_tools_version: String,

    /// Only required if the project actually has plugin/cpp or
    /// plugin/rust — the build engine's existing per-stage checks
    /// already skip NDK-dependent stages when those dirs don't exist,
    /// so an absent ndkVersion here is fine for pure-Kotlin plugins.
    #[serde(rename = "ndkVersion", default)]
    pub ndk_version: Option<String>,

    /// Which device CPU architectures the plugin's native (C++/Rust)
    /// code should be compiled for. Only relevant to plugins that
    /// actually have plugin/cpp or plugin/rust — a pure-Kotlin
    /// plugin runs on every architecture automatically since Kotlin
    /// bytecode has no ABI. Defaults to `["arm64-v8a"]` alone for
    /// backward compatibility with plugin.json files written before
    /// this field existed — NOT all four ABIs, because silently
    /// growing an existing plugin's build time/output size on upgrade
    /// would be a surprising side effect of an unrelated tooling
    /// change. Authors who want broader device coverage must opt in
    /// explicitly by listing the ABIs they want.
    #[serde(rename = "abis", default = "default_abis")]
    pub abis: Vec<String>,
}

fn default_abis() -> Vec<String> {
    vec!["arm64-v8a".to_string()]
}

impl PluginRequirements {
    /// Resolves targetSdk to compileSdk when the field was omitted —
    /// the common case where a plugin targets whatever it was
    /// compiled against.
    pub fn effective_target_sdk(&self) -> u32 {
        self.target_sdk.unwrap_or(self.compile_sdk)
    }

    /// Parses `abis` into the strongly-typed form the build engine
    /// actually needs, dropping (with no error) any string that isn't
    /// one of the four recognized ABI names — a typo here should
    /// degrade to "that one ABI isn't built" rather than fail the
    /// whole manifest load, matching this crate's general policy of
    /// tolerating unrecognized values in permissive fields.
    pub fn target_abis(&self) -> Vec<crate_abi::Abi> {
        self.abis
            .iter()
            .filter_map(|s| crate_abi::Abi::parse(s))
            .collect()
    }
}

/// Re-exported so plugin.json's ABI strings and the build engine's ABI
/// enum share exactly one parser/serializer — defined in
/// youki-toolchain since that's also where clang/rustc target-triple
/// mapping for each ABI lives, and duplicating the enum here would
/// risk the two drifting out of sync.
pub mod crate_abi {
    pub use youki_toolchain::Abi;
}

impl Default for PluginRequirements {
    fn default() -> Self {
        PluginRequirements {
            compile_sdk: default_compile_sdk(),
            min_sdk: default_min_sdk(),
            target_sdk: None,
            build_tools_version: default_build_tools_version(),
            ndk_version: None,
            abis: default_abis(),
        }
    }
}

fn default_compile_sdk() -> u32 {
    35
}

fn default_min_sdk() -> u32 {
    26
}

fn default_build_tools_version() -> String {
    "35.0.0".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    #[serde(rename = "pluginId", default = "default_plugin_id")]
    pub plugin_id: String,

    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,

    #[serde(default)]
    pub version: Option<String>,

    #[serde(default)]
    pub permissions: Vec<String>,

    pub entry: Option<PluginEntry>,

    /// Optional SDK/NDK version pins. Defaults to compileSdk 34 /
    /// build-tools 34.0.0 / no NDK requirement when absent, so old
    /// plugin.json files without this field still resolve to a sane
    /// default rather than failing to load.
    #[serde(default)]
    pub requirements: PluginRequirements,

    /// Catch-all for every field this crate doesn't model explicitly
    /// (surface, enabled, webview config, etc). Keeps us forward
    /// compatible with Youki Shell schema additions without needing a
    /// build-tool release to read them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_plugin_id() -> String {
    "com.example.plugin".to_string()
}

impl PluginManifest {
    /// Loads plugin.json from a project root. Mirrors the original
    /// Kotlin's `runCatching { ... }.getOrDefault(...)` behavior at
    /// call sites: a missing or malformed plugin.json should never
    /// crash the whole build — callers get an error they can choose
    /// to soft-fail on, matching how AAPT2 manifest generation and the
    /// packaging stage in PluginBuildEngine.kt both tolerated absence.
    pub fn load(project_dir: &Path) -> Result<Self> {
        let manifest_path = project_dir.join("plugin.json");
        let raw = fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {}", manifest_path.display()))?;
        let manifest: PluginManifest = serde_json::from_str(&raw)
            .with_context(|| format!("parsing {}", manifest_path.display()))?;
        Ok(manifest)
    }

    /// Same as `load`, but returns sensible defaults on any failure
    /// instead of propagating the error — used in build-internal spots
    /// (like AndroidManifest.xml generation for aapt2) where a missing
    /// plugin.json shouldn't abort a stage that has its own separate
    /// validation elsewhere.
    pub fn load_or_default(project_dir: &Path) -> Self {
        Self::load(project_dir).unwrap_or_else(|_| PluginManifest {
            plugin_id: default_plugin_id(),
            display_name: None,
            version: None,
            permissions: Vec::new(),
            entry: None,
            requirements: PluginRequirements::default(),
            extra: serde_json::Map::new(),
        })
    }

    /// Returns the plugin's declared dex path, made safe to join onto
    /// any base directory.
    ///
    /// SECURITY FIX: the raw `entry.dexPath` string from plugin.json
    /// was previously handed straight to `PathBuf::join()` by callers.
    /// Rust's `join()` does not stop `..` components from escaping the
    /// base directory — a plugin.json declaring
    /// `"dexPath": "../../../../tmp/evil.dex"` would cause the build
    /// engine to write a file outside the project entirely (confirmed:
    /// this actually wrote to a real path outside build_dir in
    /// testing), AND embed that same escaping path as a literal zip
    /// entry name in the final plugin.zip — a classic Zip Slip
    /// (CWE-22) payload that could let a naive unzip on the *end
    /// user's* device write outside the extraction directory.
    ///
    /// This method strips every `..`, every absolute-path root, and
    /// every empty/`.`-only component, keeping only the components
    /// that form a genuine relative descent — so
    /// `"../../../../tmp/evil.dex"` becomes `"tmp/evil.dex"` (still
    /// odd, but fully contained), and a legitimate
    /// `"lib/classes.dex"` is untouched.
    pub fn dex_path(&self) -> String {
        let raw = self
            .entry
            .as_ref()
            .map(|e| e.dex_path.clone())
            .unwrap_or_else(default_dex_path);
        sanitize_relative_path(&raw)
    }
}

/// Neutralizes path traversal and absolute-path escapes in a
/// developer-supplied relative path string, without depending on the
/// path actually existing on disk (a pure string/Path operation, safe
/// to call before any file exists — unlike `canonicalize()`, which
/// requires the path to exist and would be the wrong tool here).
fn sanitize_relative_path(raw: &str) -> String {
    use std::path::Component;

    let cleaned: PathBuf = Path::new(raw)
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part),
            // ParentDir ("..", the actual vulnerability), RootDir /
            // Prefix (absolute paths, e.g. "/etc/passwd" or a Windows
            // drive letter), and CurDir (".", harmless but pointless)
            // are all dropped rather than preserved.
            _ => None,
        })
        .collect();

    if cleaned.as_os_str().is_empty() {
        default_dex_path()
    } else {
        cleaned.to_string_lossy().replace('\\', "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn loads_minimal_manifest() {
        let dir = tempdir();
        let mut f = fs::File::create(dir.join("plugin.json")).unwrap();
        write!(f, r#"{{"pluginId": "com.example.foo"}}"#).unwrap();
        let manifest = PluginManifest::load(&dir).unwrap();
        assert_eq!(manifest.plugin_id, "com.example.foo");
        assert_eq!(manifest.dex_path(), "classes.dex");
    }

    #[test]
    fn custom_dex_path_is_respected() {
        let dir = tempdir();
        let mut f = fs::File::create(dir.join("plugin.json")).unwrap();
        write!(
            f,
            r#"{{"pluginId": "com.example.foo", "entry": {{"type": "native_fragment", "dexPath": "lib/classes.dex"}}}}"#
        )
        .unwrap();
        let manifest = PluginManifest::load(&dir).unwrap();
        assert_eq!(manifest.dex_path(), "lib/classes.dex");
    }

    #[test]
    fn defaults_to_arm64_only_when_abis_omitted() {
        let dir = tempdir();
        let mut f = fs::File::create(dir.join("plugin.json")).unwrap();
        write!(f, r#"{{"pluginId": "com.example.foo"}}"#).unwrap();
        let manifest = PluginManifest::load(&dir).unwrap();
        assert_eq!(manifest.requirements.abis, vec!["arm64-v8a".to_string()]);
        assert_eq!(manifest.requirements.target_abis().len(), 1);
    }

    #[test]
    fn parses_multiple_abis() {
        let dir = tempdir();
        let mut f = fs::File::create(dir.join("plugin.json")).unwrap();
        write!(
            f,
            r#"{{"pluginId": "com.example.foo", "requirements": {{"abis": ["arm64-v8a", "armeabi-v7a", "x86_64"]}}}}"#
        )
        .unwrap();
        let manifest = PluginManifest::load(&dir).unwrap();
        assert_eq!(manifest.requirements.target_abis().len(), 3);
    }

    #[test]
    fn unrecognized_abi_string_is_silently_dropped() {
        let dir = tempdir();
        let mut f = fs::File::create(dir.join("plugin.json")).unwrap();
        write!(
            f,
            r#"{{"pluginId": "com.example.foo", "requirements": {{"abis": ["arm64-v8a", "not-a-real-abi"]}}}}"#
        )
        .unwrap();
        let manifest = PluginManifest::load(&dir).unwrap();
        // The typo doesn't fail the whole manifest — it just doesn't
        // contribute a build target.
        assert_eq!(manifest.requirements.target_abis().len(), 1);
    }

    // --- Security: dexPath path-traversal sanitization ---
    // Regression tests for a real vulnerability found in review: a
    // plugin.json declaring a dexPath with "../" components could
    // make the build engine write files outside build_dir entirely,
    // and could embed an escaping path as a literal zip entry name in
    // the final plugin.zip (a Zip Slip / CWE-22 payload). Confirmed via
    // an actual build before this fix: the file really did land
    // outside the project directory on disk.

    #[test]
    fn dex_path_strips_parent_dir_traversal() {
        let dir = tempdir();
        let mut f = fs::File::create(dir.join("plugin.json")).unwrap();
        write!(
            f,
            r#"{{"pluginId": "com.example.foo", "entry": {{"type": "native_fragment", "dexPath": "../../../../tmp/evil.dex"}}}}"#
        )
        .unwrap();
        let manifest = PluginManifest::load(&dir).unwrap();
        let sanitized = manifest.dex_path();
        assert!(!sanitized.contains(".."), "sanitized path must not contain any '..': {sanitized}");
        assert_eq!(sanitized, "tmp/evil.dex");
    }

    #[test]
    fn dex_path_strips_absolute_path_root() {
        let dir = tempdir();
        let mut f = fs::File::create(dir.join("plugin.json")).unwrap();
        write!(
            f,
            r#"{{"pluginId": "com.example.foo", "entry": {{"type": "native_fragment", "dexPath": "/etc/passwd"}}}}"#
        )
        .unwrap();
        let manifest = PluginManifest::load(&dir).unwrap();
        let sanitized = manifest.dex_path();
        assert!(!sanitized.starts_with('/'), "sanitized path must not be absolute: {sanitized}");
        assert_eq!(sanitized, "etc/passwd");
    }

    #[test]
    fn dex_path_legitimate_subdirectory_is_unaffected() {
        let dir = tempdir();
        let mut f = fs::File::create(dir.join("plugin.json")).unwrap();
        write!(
            f,
            r#"{{"pluginId": "com.example.foo", "entry": {{"type": "native_fragment", "dexPath": "lib/classes.dex"}}}}"#
        )
        .unwrap();
        let manifest = PluginManifest::load(&dir).unwrap();
        // A real, contained subdirectory must survive sanitization
        // unchanged — the fix must not be so aggressive it breaks the
        // legitimate, documented use of dexPath.
        assert_eq!(manifest.dex_path(), "lib/classes.dex");
    }

    #[test]
    fn dex_path_traversal_that_fully_cancels_out_falls_back_to_default() {
        let dir = tempdir();
        let mut f = fs::File::create(dir.join("plugin.json")).unwrap();
        write!(
            f,
            r#"{{"pluginId": "com.example.foo", "entry": {{"type": "native_fragment", "dexPath": "../../.."}}}}"#
        )
        .unwrap();
        let manifest = PluginManifest::load(&dir).unwrap();
        assert_eq!(manifest.dex_path(), "classes.dex");
    }

    fn tempdir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("youki-manifest-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
