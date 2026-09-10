//! Locates every external tool the build engine shells out to.
//!
//! The original Kotlin (`Environment.java`) hardcoded Termux-style
//! paths like `/data/data/<pkg>/files/usr/bin/...` because it only
//! ever ran inside AndroidIDE's own app sandbox. This crate instead
//! probes, in order:
//!
//!   1. An explicit override via `YOUKI_<TOOL>_PATH` env var (escape
//!      hatch for unusual installs).
//!   2. `$ANDROID_HOME` / `$ANDROID_NDK_HOME` for Android SDK/NDK tools
//!      (aapt2, d8, r8, zipalign) — these live in versioned
//!      subdirectories, so we pick the newest installed version.
//!   3. `$PATH` via `which`, for general-purpose tools that aren't
//!      part of the Android SDK (kotlinc, clang++, cargo, cmake).
//!
//! This is what makes the same binary work unmodified on both Termux
//! (where everything sits under $PREFIX) and a normal desktop Linux
//! install (where tools are wherever the user's package manager or
//! Android Studio put them).

use anyhow::{anyhow, Result};
use std::env;
use std::path::{Path, PathBuf};

/// The four Android ABIs still shipped by a modern NDK. `armeabi-v7a`
/// covers old 32-bit ARM devices (still common on very low-end/budget
/// hardware); `x86`/`x86_64` cover emulators and the rare x86 tablet.
/// Confirmed exact triples/prefixes against Android's own NDK docs
/// ("Use the NDK with other build systems") and the `ndk-build` crate's
/// target table — not guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Abi {
    Arm64V8a,
    ArmeabiV7a,
    X86,
    X86_64,
}

impl Abi {
    pub fn all() -> [Abi; 4] {
        [Abi::Arm64V8a, Abi::ArmeabiV7a, Abi::X86, Abi::X86_64]
    }

    /// The exact string used in plugin.json's `requirements.abis` array
    /// and as the jniLibs-style output subfolder name.
    pub fn as_str(self) -> &'static str {
        match self {
            Abi::Arm64V8a => "arm64-v8a",
            Abi::ArmeabiV7a => "armeabi-v7a",
            Abi::X86 => "x86",
            Abi::X86_64 => "x86_64",
        }
    }

    pub fn parse(s: &str) -> Option<Abi> {
        match s {
            "arm64-v8a" => Some(Abi::Arm64V8a),
            "armeabi-v7a" => Some(Abi::ArmeabiV7a),
            "x86" => Some(Abi::X86),
            "x86_64" => Some(Abi::X86_64),
            _ => None,
        }
    }

    /// clang's `--target=` triple, WITHOUT the minSdk suffix (caller
    /// appends `{min_sdk}` themselves — this varies per build).
    /// NOTE: 32-bit ARM is the one irregular case — its clang triple
    /// prefix is `armv7a-linux-androideabi`, NOT `arm-linux-androideabi`
    /// (that second form is what the *binutils*, not clang, use — an
    /// easy mix-up confirmed via Android's own NDK documentation).
    pub fn clang_target_prefix(self) -> &'static str {
        match self {
            Abi::Arm64V8a => "aarch64-linux-android",
            Abi::ArmeabiV7a => "armv7a-linux-androideabi",
            Abi::X86 => "i686-linux-android",
            Abi::X86_64 => "x86_64-linux-android",
        }
    }

    /// Rust target triple, for `cargo ndk -t <this>` / `rustc --target`.
    pub fn rust_target_triple(self) -> &'static str {
        match self {
            Abi::Arm64V8a => "aarch64-linux-android",
            Abi::ArmeabiV7a => "armv7-linux-androideabi",
            Abi::X86 => "i686-linux-android",
            Abi::X86_64 => "x86_64-linux-android",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolchainPaths {
    pub javac: Option<PathBuf>,
    pub kotlinc: Option<PathBuf>,
    pub aapt2: Option<PathBuf>,
    pub d8: Option<PathBuf>,
    pub r8: Option<PathBuf>,
    pub zipalign: Option<PathBuf>,
    pub android_jar: Option<PathBuf>,
    pub clangxx: Option<PathBuf>,
    pub ndk_root: Option<PathBuf>,
    pub rustc: Option<PathBuf>,
    pub cargo_ndk: Option<PathBuf>,
    pub cargo_clippy: Option<PathBuf>,
    pub cmake: Option<PathBuf>,
    pub qt_cmake_toolchain_file: Option<PathBuf>,
}

/// One missing tool, reported with enough context for the CLI to print
/// a specific "here's how to fix it" message rather than a bare
/// "command not found".
#[derive(Debug, Clone)]
pub struct MissingTool {
    pub name: &'static str,
    pub hint: &'static str,
}

impl ToolchainPaths {
    /// Probes the environment for every tool. Never fails outright —
    /// mirrors the original engine's per-stage `failMissingTool`
    /// behavior: a stage that isn't needed (e.g. no Rust sources) is
    /// allowed to have no Rust toolchain at all. Missing tools are
    /// discovered lazily, at the point a stage that needs them runs.
    pub fn discover(android_home: Option<&Path>, ndk_home: Option<&Path>) -> Self {
        let android_home = android_home
            .map(PathBuf::from)
            .or_else(|| env::var_os("ANDROID_HOME").map(PathBuf::from))
            .or_else(|| env::var_os("ANDROID_SDK_ROOT").map(PathBuf::from));

        let ndk_home = ndk_home
            .map(PathBuf::from)
            .or_else(|| env::var_os("ANDROID_NDK_HOME").map(PathBuf::from));

        ToolchainPaths {
            javac: find_on_path("javac"),
            kotlinc: env_override("KOTLINC").or_else(|| find_on_path("kotlinc")),
            aapt2: env_override("AAPT2").or_else(|| {
                android_home
                    .as_deref()
                    .and_then(|home| newest_build_tool(home, "aapt2"))
            }),
            d8: env_override("D8").or_else(|| {
                android_home
                    .as_deref()
                    .and_then(|home| newest_build_tool(home, "d8"))
            }),
            r8: env_override("R8").or_else(|| find_on_path("r8")),
            zipalign: env_override("ZIPALIGN").or_else(|| {
                android_home
                    .as_deref()
                    .and_then(|home| newest_build_tool(home, "zipalign"))
            }),
            android_jar: env_override("ANDROID_JAR").or_else(|| {
                android_home
                    .as_deref()
                    .and_then(newest_platform_android_jar)
            }),
            clangxx: env_override("CLANGXX").or_else(|| {
                ndk_home
                    .as_deref()
                    .and_then(find_ndk_clangxx)
                    .or_else(|| find_on_path("clang++"))
            }),
            ndk_root: ndk_home.clone(),
            rustc: find_on_path("rustc"),
            cargo_ndk: env_override("CARGO_NDK").or_else(|| find_on_path("cargo-ndk")),
            cargo_clippy: find_on_path("cargo-clippy"),
            cmake: find_on_path("cmake"),
            qt_cmake_toolchain_file: env::var_os("QT_ANDROID_TOOLCHAIN")
                .map(PathBuf::from)
                .filter(|p| p.exists()),
        }
    }

    pub fn missing_for_kotlin(&self) -> Vec<MissingTool> {
        let mut missing = Vec::new();
        if self.kotlinc.is_none() {
            missing.push(MissingTool {
                name: "kotlinc",
                hint: "install Kotlin (pkg install kotlin on Termux, or add it to PATH)",
            });
        }
        if self.android_jar.is_none() {
            missing.push(MissingTool {
                name: "android.jar",
                hint: "set ANDROID_HOME to a valid Android SDK, or run `youki sdk install platform-<api>`",
            });
        }
        missing
    }
}

fn env_override(tool_env_suffix: &str) -> Option<PathBuf> {
    env::var_os(format!("YOUKI_{tool_env_suffix}_PATH"))
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

fn find_on_path(bin: &str) -> Option<PathBuf> {
    which::which(bin).ok()
}

/// Android SDK build-tools ship as `<sdk>/build-tools/<version>/<tool>`
/// with multiple versions often installed side by side. Picks the
/// highest version directory that actually contains the requested
/// tool, so a partially-installed older version doesn't win.
fn newest_build_tool(android_home: &Path, tool: &str) -> Option<PathBuf> {
    let build_tools_dir = android_home.join("build-tools");
    let mut versions: Vec<_> = std::fs::read_dir(&build_tools_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    versions.sort();
    versions
        .into_iter()
        .rev()
        .find_map(|dir| {
            let candidate = dir.join(tool);
            candidate.exists().then_some(candidate)
        })
}

/// Same "newest wins" logic as `newest_build_tool`, but for
/// `<sdk>/platforms/android-<api>/android.jar`.
fn newest_platform_android_jar(android_home: &Path) -> Option<PathBuf> {
    let platforms_dir = android_home.join("platforms");
    let mut platforms: Vec<_> = std::fs::read_dir(&platforms_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    platforms.sort();
    platforms.into_iter().rev().find_map(|dir| {
        let jar = dir.join("android.jar");
        jar.exists().then_some(jar)
    })
}

/// NDK clang++ lives at a host-specific triple path, e.g.
/// `toolchains/llvm/prebuilt/linux-x86_64/bin/clang++`. Only the Linux
/// host triple is probed since Termux and desktop Linux are the two
/// supported hosts for this tool.
fn find_ndk_clangxx(ndk_home: &Path) -> Option<PathBuf> {
    let candidate = ndk_home
        .join("toolchains/llvm/prebuilt/linux-x86_64/bin/clang++");
    candidate.exists().then_some(candidate)
}

pub fn require(tool: &Option<PathBuf>, missing: MissingTool) -> Result<PathBuf> {
    tool.clone()
        .ok_or_else(|| anyhow!("required tool not found: {} ({})", missing.name, missing.hint))
}
