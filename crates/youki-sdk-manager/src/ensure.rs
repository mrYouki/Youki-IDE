//! The actual "does the developer already have what they need, and if
//! not, get it automatically" flow requested for `youki build`.
//!
//! This is intentionally a thin layer on top of [`crate::SdkManager`]:
//! it only decides WHAT needs installing (by comparing plugin.json's
//! `requirements` against what's on disk) and drives the download; it
//! does not know anything about compilers or the build pipeline
//! itself — that stays youki-toolchain's and youki-build-engine's job.

use crate::SdkManager;
use anyhow::Result;
use std::fmt;
use youki_manifest::PluginRequirements;

#[derive(Debug, Clone)]
pub enum RequiredComponent {
    Platform(u32),
    BuildTools(String),
    Ndk(String),
}

impl fmt::Display for RequiredComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequiredComponent::Platform(api) => write!(f, "platform-{api} (android.jar)"),
            RequiredComponent::BuildTools(v) => write!(f, "build-tools {v}"),
            RequiredComponent::Ndk(v) => write!(f, "NDK {v}"),
        }
    }
}

/// Diffs `requirements` against what's already installed under
/// `mgr.android_home`. `needs_ndk` should be true only when the
/// project actually has plugin/cpp or plugin/rust — a plugin with
/// neither has no reason to force an NDK download just because
/// plugin.json happens to mention a version.
pub fn missing_components(
    mgr: &SdkManager,
    requirements: &PluginRequirements,
    needs_ndk: bool,
) -> Vec<RequiredComponent> {
    let mut missing = Vec::new();

    if !mgr.is_platform_installed(requirements.compile_sdk) {
        missing.push(RequiredComponent::Platform(requirements.compile_sdk));
    }
    if !mgr.is_build_tools_installed(&requirements.build_tools_version) {
        missing.push(RequiredComponent::BuildTools(
            requirements.build_tools_version.clone(),
        ));
    }
    if needs_ndk {
        if let Some(ndk_version) = &requirements.ndk_version {
            if !mgr.is_ndk_installed(ndk_version) {
                missing.push(RequiredComponent::Ndk(ndk_version.clone()));
            }
        }
    }

    missing
}

/// Installs every missing component in order, reporting progress
/// through `on_progress` per-component as (component, bytes_done,
/// bytes_total). Stops at the first failure — a half-installed SDK is
/// a normal, resumable state (re-running just retries what's still
/// missing), so there's no cleanup-on-failure logic needed here.
pub fn install_missing(
    mgr: &SdkManager,
    missing: &[RequiredComponent],
    mut on_progress: impl FnMut(&RequiredComponent, u64, u64),
) -> Result<()> {
    for component in missing {
        match component {
            RequiredComponent::Platform(api) => {
                mgr.install_platform(*api, None, |done, total| on_progress(component, done, total))?;
            }
            RequiredComponent::BuildTools(version) => {
                mgr.install_build_tools(version, None, |done, total| {
                    on_progress(component, done, total)
                })?;
            }
            RequiredComponent::Ndk(version) => {
                mgr.install_ndk(version, |done, total| on_progress(component, done, total))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn reports_nothing_missing_when_all_present() {
        let dir = tempdir();
        let mgr = SdkManager::new(&dir);
        std::fs::create_dir_all(dir.join("platforms/android-34")).unwrap();
        std::fs::write(dir.join("platforms/android-34/android.jar"), "x").unwrap();
        std::fs::create_dir_all(dir.join("build-tools/34.0.0")).unwrap();

        let req = PluginRequirements {
            compile_sdk: 34,
            build_tools_version: "34.0.0".to_string(),
            ndk_version: None,
            ..Default::default()
        };
        assert!(missing_components(&mgr, &req, false).is_empty());
    }

    #[test]
    fn reports_missing_platform_and_build_tools() {
        let dir = tempdir();
        let mgr = SdkManager::new(&dir);
        let req = PluginRequirements {
            compile_sdk: 34,
            build_tools_version: "34.0.0".to_string(),
            ndk_version: None,
            ..Default::default()
        };
        let missing = missing_components(&mgr, &req, false);
        assert_eq!(missing.len(), 2);
    }

    #[test]
    fn skips_ndk_when_project_has_no_native_code() {
        let dir = tempdir();
        let mgr = SdkManager::new(&dir);
        std::fs::create_dir_all(dir.join("platforms/android-34")).unwrap();
        std::fs::write(dir.join("platforms/android-34/android.jar"), "x").unwrap();
        std::fs::create_dir_all(dir.join("build-tools/34.0.0")).unwrap();

        let req = PluginRequirements {
            compile_sdk: 34,
            build_tools_version: "34.0.0".to_string(),
            ndk_version: Some("27.2.12479018".to_string()),
            ..Default::default()
        };
        // needs_ndk = false -> even though ndkVersion is set, we
        // don't demand it for a plugin with no native sources.
        assert!(missing_components(&mgr, &req, false).is_empty());
    }

    #[test]
    fn requires_ndk_when_project_has_native_code() {
        let dir = tempdir();
        let mgr = SdkManager::new(&dir);
        std::fs::create_dir_all(dir.join("platforms/android-34")).unwrap();
        std::fs::write(dir.join("platforms/android-34/android.jar"), "x").unwrap();
        std::fs::create_dir_all(dir.join("build-tools/34.0.0")).unwrap();

        let req = PluginRequirements {
            compile_sdk: 34,
            build_tools_version: "34.0.0".to_string(),
            ndk_version: Some("27.2.12479018".to_string()),
            ..Default::default()
        };
        let missing = missing_components(&mgr, &req, true);
        assert_eq!(missing.len(), 1);
        assert!(matches!(missing[0], RequiredComponent::Ndk(_)));
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "youki-sdk-ensure-test-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
    }
}
