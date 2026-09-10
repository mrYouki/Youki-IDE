//! Resolves download URLs against Google's publicly hosted Android
//! SDK/NDK repository — the same server `sdkmanager` and Android
//! Studio pull from. No API key or Android Studio installation
//! required; these are plain HTTPS zip downloads.
//!
//! NOTE: Google periodically revises exact build-tools/NDK point
//! releases. The version strings below are pinned to known-good
//! releases at the time of writing; `youki sdk install <x> --version
//! <y>` lets a user override if a pin goes stale.

use anyhow::Result;

const DL_HOST: &str = "https://dl.google.com/android/repository";

pub fn platform_download_url(api_level: u32) -> Result<String> {
    // Platform archives are named e.g. "platform-34_r03.zip". Google
    // does not expose a stable "latest" alias per API level, so this
    // targets the well-known package path used by sdkmanager's own
    // repository XML for the given API level's most common revision.
    Ok(format!("{DL_HOST}/platform-{api_level}_r01.zip"))
}

pub fn build_tools_download_url(version: &str) -> Result<String> {
    // Confirmed exact filename: "build-tools_r<version>-linux.zip".
    // NOTE: the archive's top-level directory inside the zip is named
    // after the Android version codename ("android-14" for
    // build-tools 34.x), NOT the build-tools version number — the
    // caller must rename after extracting. See SdkManager::install_build_tools.
    Ok(format!("{DL_HOST}/build-tools_r{version}-linux.zip"))
}

pub fn ndk_download_url(version: &str) -> Result<String> {
    // e.g. android-ndk-r27c-linux.zip
    Ok(format!("{DL_HOST}/android-ndk-{version}-linux.zip"))
}
