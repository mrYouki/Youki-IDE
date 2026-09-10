//! Manages an Android SDK/NDK installation independent of Android
//! Studio or the old AndroidIDE Gradle-plugin download flow.
//!
//! Where the original AndroidIDE bundled SDK components as app assets
//! unpacked at first run (`AddAndroidJarToAssetsTask`, `SetupAapt2Task`
//! in the Gradle build-logic), this crate downloads components
//! on-demand straight from Google's own hosted package XML/zips,
//! exactly what `sdkmanager` does under the hood — so `youki sdk` has
//! no dependency on Android Studio or Gradle being present at all.
//!
//! Layout on disk (both Termux and desktop Linux):
//! ```text
//! $ANDROID_HOME/
//!   platforms/android-<api>/android.jar
//!   build-tools/<version>/{aapt2,d8,r8,zipalign}
//!   ndk/<version>/toolchains/llvm/prebuilt/linux-x86_64/bin/clang++
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub mod ensure;
pub mod repository;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledComponent {
    pub id: String,
    pub version: String,
    pub path: PathBuf,
}

pub struct SdkManager {
    pub android_home: PathBuf,
}

impl SdkManager {
    pub fn new(android_home: impl Into<PathBuf>) -> Self {
        Self {
            android_home: android_home.into(),
        }
    }

    /// Default install location, matching Termux `$HOME/android-sdk`
    /// convention from the original `Environment.java` so existing
    /// AndroidIDE installs are picked up without any extra flag.
    pub fn default_location() -> PathBuf {
        let home = dirs_home();
        home.join("android-sdk")
    }

    pub fn is_platform_installed(&self, api_level: u32) -> bool {
        self.android_home
            .join(format!("platforms/android-{api_level}/android.jar"))
            .exists()
    }

    pub fn is_build_tools_installed(&self, version: &str) -> bool {
        self.android_home
            .join(format!("build-tools/{version}"))
            .exists()
    }

    pub fn is_ndk_installed(&self, version: &str) -> bool {
        self.android_home.join(format!("ndk/{version}")).exists()
    }

    pub fn list_installed(&self) -> Vec<InstalledComponent> {
        let mut out = Vec::new();
        self.list_versioned_dir("platforms", "platform", &mut out);
        self.list_versioned_dir("build-tools", "build-tools", &mut out);
        self.list_versioned_dir("ndk", "ndk", &mut out);
        out
    }

    fn list_versioned_dir(&self, subdir: &str, id_prefix: &str, out: &mut Vec<InstalledComponent>) {
        let dir = self.android_home.join(subdir);
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                if entry.path().is_dir() {
                    let version = entry.file_name().to_string_lossy().to_string();
                    out.push(InstalledComponent {
                        id: format!("{id_prefix}-{version}"),
                        version,
                        path: entry.path(),
                    });
                }
            }
        }
    }

    /// Downloads and unpacks a platform's android.jar. `api_level`
    /// Downloads and unpacks a platform's android.jar. `api_level`
    /// selects the package (e.g. 34 -> "android-34"). Progress is
    /// reported through `on_progress` as (bytes_downloaded, total_bytes).
    ///
    /// IMPORTANT: platform-<N>_r0X.zip extracts into ITS OWN nested
    /// top-level folder (e.g. "android-13/"), not into the destination
    /// directly — confirmed against Google's real archives. Extracting
    /// straight into `dest` and checking for android.jar there would
    /// silently fail; this unpacks into a temp dir and moves the real
    /// contents up one level.
    pub fn install_platform(
        &self,
        api_level: u32,
        mut on_progress: impl FnMut(u64, u64),
    ) -> Result<()> {
        let dest = self.android_home.join(format!("platforms/android-{api_level}"));
        fs::create_dir_all(&dest)?;

        let url = repository::platform_download_url(api_level)?;
        let archive_path = self
            .android_home
            .join(format!("_platform-{api_level}.zip"));
        download_with_progress(&url, &archive_path, &mut on_progress)?;

        let extract_tmp = self
            .android_home
            .join(format!("_platform-{api_level}-extract-tmp"));
        let _ = fs::remove_dir_all(&extract_tmp);
        fs::create_dir_all(&extract_tmp)?;
        unzip_to(&archive_path, &extract_tmp)?;
        fs::remove_file(&archive_path).ok();

        promote_single_nested_dir(&extract_tmp, &dest)?;
        let _ = fs::remove_dir_all(&extract_tmp);

        anyhow::ensure!(
            dest.join("android.jar").exists(),
            "downloaded platform archive did not contain android.jar (Google may have renamed this release; try `youki sdk install-platform {api_level} --version <exact-r-number>`)"
        );
        Ok(())
    }

    /// Downloads and unpacks build-tools (aapt2, d8, r8, zipalign).
    ///
    /// IMPORTANT: build-tools_r<N>-linux.zip also extracts into its own
    /// nested top-level folder named after the Android codename (e.g.
    /// "android-14/" for build-tools 34.x) — NOT a folder matching the
    /// build-tools version number itself. Same promote-up-one-level
    /// handling as install_platform.
    pub fn install_build_tools(
        &self,
        version: &str,
        mut on_progress: impl FnMut(u64, u64),
    ) -> Result<()> {
        let dest = self.android_home.join(format!("build-tools/{version}"));
        fs::create_dir_all(&dest)?;

        let url = repository::build_tools_download_url(version)?;
        let archive_path = self
            .android_home
            .join(format!("_build_tools-{version}.zip"));
        download_with_progress(&url, &archive_path, &mut on_progress)?;

        let extract_tmp = self
            .android_home
            .join(format!("_build_tools-{version}-extract-tmp"));
        let _ = fs::remove_dir_all(&extract_tmp);
        fs::create_dir_all(&extract_tmp)?;
        unzip_to(&archive_path, &extract_tmp)?;
        fs::remove_file(&archive_path).ok();

        promote_single_nested_dir(&extract_tmp, &dest)?;
        let _ = fs::remove_dir_all(&extract_tmp);

        make_executable(&dest.join("aapt2"));
        make_executable(&dest.join("zipalign"));

        anyhow::ensure!(
            dest.join("aapt2").exists(),
            "downloaded build-tools archive did not contain aapt2 (Google may have renamed this release; try `youki sdk install-build-tools {version} --version <exact-r-number>`)"
        );
        Ok(())
    }

    /// Downloads and unpacks the NDK. This is the large one (500MB+),
    /// so progress reporting matters most here for user feedback.
    pub fn install_ndk(&self, version: &str, mut on_progress: impl FnMut(u64, u64)) -> Result<()> {
        let dest_parent = self.android_home.join("ndk");
        fs::create_dir_all(&dest_parent)?;

        let url = repository::ndk_download_url(version)?;
        let archive_path = dest_parent.join("_ndk.zip");
        download_with_progress(&url, &archive_path, &mut on_progress)?;
        unzip_to(&archive_path, &dest_parent)?;
        fs::remove_file(&archive_path).ok();

        // NDK zips extract into a versioned top-level dir already
        // (e.g. android-ndk-r27); normalize to `ndk/<version>` so
        // youki-toolchain's lookup logic doesn't need to guess names.
        let extracted_dir = fs::read_dir(&dest_parent)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.is_dir() && p.file_name().map(|n| n != version).unwrap_or(false));
        if let Some(extracted) = extracted_dir {
            let target = dest_parent.join(version);
            if !target.exists() {
                fs::rename(&extracted, &target)?;
            }
        }
        Ok(())
    }
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o111);
            let _ = fs::set_permissions(path, perms);
        }
    }
}

/// Google's platform-*/build-tools_r* archives extract into a single
/// nested top-level directory whose name doesn't match anything the
/// caller predicted in advance (e.g. build-tools 34.0.0 unpacks into
/// "android-14/", not "34.0.0/"). This moves the CONTENTS of that one
/// nested directory up into `dest`, so callers get a predictable path
/// regardless of what Google happened to name the inner folder for a
/// given release.
///
/// If the archive extracted flat (no single nested dir — some Google
/// archives do this already), this is a no-op: `extracted_root`'s
/// contents are just moved as-is.
fn promote_single_nested_dir(extracted_root: &Path, dest: &Path) -> Result<()> {
    let entries: Vec<_> = fs::read_dir(extracted_root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();

    let source_dir = if entries.len() == 1 && entries[0].is_dir() {
        // Exactly one top-level directory and nothing else alongside
        // it — this is the "android-14/"-style nesting.
        entries[0].clone()
    } else {
        // Already flat.
        extracted_root.to_path_buf()
    };

    for entry in fs::read_dir(&source_dir)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if target.exists() {
            if target.is_dir() {
                fs::remove_dir_all(&target)?;
            } else {
                fs::remove_file(&target)?;
            }
        }
        fs::rename(entry.path(), target)?;
    }
    Ok(())
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn download_with_progress(
    url: &str,
    dest: &Path,
    on_progress: &mut impl FnMut(u64, u64),
) -> Result<()> {
    let response = ureq::get(url)
        .call()
        .with_context(|| format!("requesting {url}"))?;
    let total: u64 = response
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let mut file = fs::File::create(dest)?;
    let mut downloaded: u64 = 0;
    let mut reader = response.into_reader();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = std::io::Read::read(&mut reader, &mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;
        on_progress(downloaded, total);
    }
    Ok(())
}

fn unzip_to(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let file = fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let out_path = match entry.enclosed_name() {
            Some(name) => dest_dir.join(name),
            None => continue,
        };
        if entry.name().ends_with('/') {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out_file = fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out_file)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                let _ = fs::set_permissions(&out_path, fs::Permissions::from_mode(mode));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "youki-sdk-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_zip_with_nested_dir(zip_path: &Path, nested_dir_name: &str, files: &[(&str, &str)]) {
        let file = fs::File::create(zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::FileOptions::default();
        for (name, content) in files {
            writer
                .start_file(format!("{nested_dir_name}/{name}"), options)
                .unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    /// Reproduces the real-world shape confirmed via Google's own
    /// archives: build-tools_r34-linux.zip's actual top-level folder
    /// is "android-14/", not "34.0.0/". Verifies our promote step
    /// produces the flat, predictable layout callers expect regardless
    /// of that mismatched inner name.
    #[test]
    fn promotes_mismatched_nested_dir_name() {
        let work = tempdir("promote-build-tools");
        let zip_path = work.join("build-tools_r34-linux.zip");
        make_zip_with_nested_dir(
            &zip_path,
            "android-14",
            &[("aapt2", "fake-aapt2-binary"), ("d8", "fake-d8-binary")],
        );

        let extract_tmp = work.join("extract-tmp");
        fs::create_dir_all(&extract_tmp).unwrap();
        unzip_to(&zip_path, &extract_tmp).unwrap();

        // Confirm the nesting assumption itself before testing the fix
        // — if this fails, the test fixture is wrong, not the code.
        assert!(extract_tmp.join("android-14/aapt2").exists());

        let dest = work.join("build-tools/34.0.0");
        fs::create_dir_all(&dest).unwrap();
        promote_single_nested_dir(&extract_tmp, &dest).unwrap();

        assert!(dest.join("aapt2").exists(), "aapt2 should be promoted to dest root");
        assert!(dest.join("d8").exists());
        assert!(!dest.join("android-14").exists(), "nested dir itself shouldn't survive");
    }

    #[test]
    fn promotes_platform_zip_nested_dir() {
        let work = tempdir("promote-platform");
        let zip_path = work.join("platform-33_r02.zip");
        make_zip_with_nested_dir(&zip_path, "android-13", &[("android.jar", "fake-jar-bytes")]);

        let extract_tmp = work.join("extract-tmp");
        fs::create_dir_all(&extract_tmp).unwrap();
        unzip_to(&zip_path, &extract_tmp).unwrap();

        let dest = work.join("platforms/android-33");
        fs::create_dir_all(&dest).unwrap();
        promote_single_nested_dir(&extract_tmp, &dest).unwrap();

        assert!(dest.join("android.jar").exists());
    }

    /// Some Google archives (varies by release) extract flat with no
    /// nesting at all. The promote step must be a safe no-op in that
    /// case rather than assuming nesting always exists.
    #[test]
    fn handles_already_flat_archive() {
        let work = tempdir("promote-flat");
        let extract_tmp = work.join("extract-tmp");
        fs::create_dir_all(&extract_tmp).unwrap();
        fs::write(extract_tmp.join("android.jar"), "fake-jar-bytes").unwrap();

        let dest = work.join("platforms/android-30");
        fs::create_dir_all(&dest).unwrap();
        promote_single_nested_dir(&extract_tmp, &dest).unwrap();

        assert!(dest.join("android.jar").exists());
    }
}
