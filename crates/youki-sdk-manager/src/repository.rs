//! Resolves download URLs against Google's publicly hosted Android
//! SDK/NDK repository — the same server `sdkmanager` and Android
//! Studio pull from. No API key or Android Studio installation
//! required; these are plain HTTPS zip downloads.
//!
//! FIX: this used to build a platform/build-tools URL by guessing a
//! hardcoded revision suffix (`_r01`) straight from the API level or
//! version string, with no verification the guessed file actually
//! exists. Google's real per-API-level revision has no predictable
//! pattern (API 28 shipped as `platform-28_r06.zip`, other levels as
//! `_r01`/`_r02`/`_r03`...), so a hardcoded guess is wrong for most API
//! levels and silently produces a 404 partway through `youki build`.
//!
//! Every function here now genuinely resolves a URL that exists,
//! instead of guessing one: it issues a lightweight `HEAD` request
//! (no body downloaded) against each candidate revision suffix in
//! turn, low numbers first, and returns the first one that responds
//! 200. This trades one or a handful of small HEAD requests for
//! never handing the caller a URL that turns out to 404 after a
//! multi-hundred-megabyte download has already started.

use anyhow::{Context, Result};

const DL_HOST: &str = "https://dl.google.com/android/repository";

/// How many revision numbers to try before giving up. Every real
/// platform/build-tools release observed to date uses a single-digit
/// revision; this is deliberately generous headroom above that, not a
/// tight fit to today's known values.
const MAX_REVISION_PROBE: u32 = 20;

/// True if a HEAD request against `url` gets back a real (2xx) response.
/// Any network error or non-2xx status is treated as "not this one" —
/// callers move on to the next candidate rather than treating a single
/// probe failure as fatal.
fn url_exists(url: &str) -> bool {
    ureq::head(url)
        .call()
        .map(|resp| resp.status() / 100 == 2)
        .unwrap_or(false)
}

/// Resolves a platform archive's real download URL by probing
/// `platform-<api_level>_r01.zip`, `_r02.zip`, ... in order and
/// returning the first one that actually exists on Google's server.
///
/// If `explicit_revision` is provided (the `--version` override, e.g.
/// `"6"` for `platform-28_r06.zip`'s `06` — accepts either `"6"` or
/// `"06"`), that exact filename is used directly with no probing, on
/// the assumption the caller looked up the correct number themselves
/// (e.g. from https://dl.google.com/android/repository/repository2-1.xml).
pub fn platform_download_url(api_level: u32, explicit_revision: Option<&str>) -> Result<String> {
    if let Some(rev) = explicit_revision {
        let rev_num: u32 = rev
            .parse()
            .with_context(|| format!("--version '{rev}' is not a plain revision number"))?;
        return Ok(format!("{DL_HOST}/platform-{api_level}_r{rev_num:02}.zip"));
    }

    for rev in 1..=MAX_REVISION_PROBE {
        let url = format!("{DL_HOST}/platform-{api_level}_r{rev:02}.zip");
        if url_exists(&url) {
            return Ok(url);
        }
    }

    anyhow::bail!(
        "could not find a platform-{api_level} archive on Google's server (checked revisions \
         r01 through r{MAX_REVISION_PROBE:02}). This API level may not exist, or its revision \
         is higher than this tool currently checks — look it up at \
         https://dl.google.com/android/repository/repository2-1.xml and pass the exact \
         revision with `youki sdk install-platform {api_level} --version <N>`."
    )
}

/// Resolves build-tools' real download URL the same way — trying the
/// modern naming (`-linux.zip`, no architecture suffix) first, then
/// falling back to older forms that spelled out `-linux-x86` or used
/// an underscore instead of a hyphen before `linux`.
///
/// `explicit_revision`, if provided, replaces `version` outright (an
/// escape hatch for when the default guess turns out wrong) — it
/// doesn't change which naming patterns get tried, just which version
/// string is substituted into them.
pub fn build_tools_download_url(version: &str, explicit_revision: Option<&str>) -> Result<String> {
    let version = explicit_revision.unwrap_or(version);

    let candidates = [
        format!("{DL_HOST}/build-tools_r{version}-linux.zip"),
        format!("{DL_HOST}/build-tools_r{version}_linux.zip"),
        format!("{DL_HOST}/build-tools_r{version}-linux-x86.zip"),
    ];

    for url in &candidates {
        if url_exists(url) {
            return Ok(url.clone());
        }
    }

    anyhow::bail!(
        "could not find a build-tools {version} archive on Google's server under any known \
         naming pattern. Confirm the exact version at \
         https://dl.google.com/android/repository/repository2-1.xml — it must match exactly, \
         including all three version components (e.g. \"34.0.0\", not \"34\" or \"34.0\")."
    )
}

/// Resolves the NDK's real download URL. NDK version strings (e.g.
/// `"27.2.12479018"` or the classic `"r27c"` form) are supplied by the
/// caller already fully specifying the release, so — unlike platform
/// and build-tools — there's no numeric revision to guess. This still
/// probes both known filename shapes (with and without the explicit
/// `-x86_64` architecture suffix; both have shipped for different NDK
/// eras) rather than assuming one.
pub fn ndk_download_url(version: &str) -> Result<String> {
    let candidates = [
        format!("{DL_HOST}/android-ndk-{version}-linux.zip"),
        format!("{DL_HOST}/android-ndk-{version}-linux-x86_64.zip"),
    ];

    for url in &candidates {
        if url_exists(url) {
            return Ok(url.clone());
        }
    }

    anyhow::bail!(
        "could not find an NDK archive for version '{version}' on Google's server. Confirm the \
         exact version string at https://developer.android.com/ndk/downloads — it must match \
         exactly, including the point-release letter if the classic \"r27c\"-style name is used."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_explicit_revision_skips_probing() {
        // No network access needed: an explicit revision is used
        // as-is, so this must not attempt any HEAD request.
        let url = platform_download_url(28, Some("6")).unwrap();
        assert_eq!(url, format!("{DL_HOST}/platform-28_r06.zip"));
    }

    #[test]
    fn platform_explicit_revision_accepts_pre_padded_input() {
        let url = platform_download_url(28, Some("06")).unwrap();
        assert_eq!(url, format!("{DL_HOST}/platform-28_r06.zip"));
    }

    #[test]
    fn platform_explicit_revision_rejects_non_numeric_input() {
        assert!(platform_download_url(28, Some("r06")).is_err());
    }
}
