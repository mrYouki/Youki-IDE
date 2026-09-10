//! Rust translation of `PluginBuildEngine.kt`.
//!
//! Presents a plugin build as ONE cohesive pipeline (progress events,
//! unified log) while internally shelling out to independent CLI
//! tools in strict order — same design as the original. Every stage
//! here corresponds 1:1 to a stage in the Kotlin version; see each
//! function's doc comment for the specific behavior it preserves.
//!
//! BUILD MODES — deliberately NOT uniform across languages (preserved
//! exactly from the original's design rationale):
//!
//!   - Kotlin/Java: Peaceful suppresses warnings, Dictator enables
//!     extra lint categories (non-fatal), Nightmare makes every
//!     warning fatal (-Werror).
//!   - C++ (clang): Peaceful silences warnings (-w), Dictator enables
//!     -Wall -Wextra (non-fatal), Nightmare adds -Werror AND
//!     sanitizers (-fsanitize=address,undefined).
//!   - Rust: Peaceful and Dictator are IDENTICAL (rustc's own lints are
//!     already strict). Only Nightmare changes anything: `-D warnings`
//!     plus `cargo clippy`.

pub mod cache;
pub mod runner;
pub mod types;

use anyhow::Result;
use cache::StageCache;
use runner::{run_tool, ToolRun};
use std::fs;
use std::path::{Path, PathBuf};
use types::{BuildEvent, BuildMode, BuildVariant};
use youki_toolchain::ToolchainPaths;

pub struct PluginBuildEngine<'a> {
    toolchain: &'a ToolchainPaths,
    project_dir: PathBuf,
    build_dir: PathBuf,
    mode: BuildMode,
    variant: BuildVariant,
    on_progress: Box<dyn FnMut(BuildEvent) + 'a>,
    cache: StageCache,

    gen_dir: PathBuf,
    compiled_res_dir: PathBuf,
    classes_dir: PathBuf,
    native_lib_dir: PathBuf,
    dex_dir: PathBuf,

    total_warnings: u32,
    total_errors: u32,

    /// Number of `engine.run()` tool invocations fired during the
    /// current stage — used only to decide whether the outer stage
    /// loop needs to synthesize its own single StageStarted/Finished
    /// announcement. A stage that shells out to a real tool one or
    /// more times (Kotlin, C++ per-ABI, Rust, dexing...) already
    /// self-announces via `run()`; a stage that produces no tool
    /// invocation at all this build — fully cache-skipped, or a
    /// no-op like "no .cpp files present" — has nothing to announce
    /// on its own, so the outer loop covers that case instead.
    tool_invocations_this_stage: u32,
}

#[allow(dead_code)]
struct StageDef<'a> {
    name: &'static str,
    applies: bool,
    action: Box<dyn FnMut(&mut PluginBuildEngine) -> Result<StageOutcome> + 'a>,
}

enum StageOutcome {
    Ok,
    /// Stage was skipped entirely because its build cache fingerprint
    /// matched the previous run and its outputs are still present.
    /// Kept distinct from `Ok` purely for progress reporting — the
    /// pipeline treats it identically to `Ok` for pass/fail purposes.
    Skipped,
    Failed,
    PackageProduced(Option<PathBuf>),
}

impl<'a> PluginBuildEngine<'a> {
    pub fn new(
        toolchain: &'a ToolchainPaths,
        project_dir: impl Into<PathBuf>,
        build_dir: impl Into<PathBuf>,
        mode: BuildMode,
        variant: BuildVariant,
        on_progress: impl FnMut(BuildEvent) + 'a,
    ) -> Result<Self> {
        let project_dir = project_dir.into();
        let build_dir = build_dir.into();

        let gen_dir = build_dir.join("gen");
        let compiled_res_dir = build_dir.join("compiled_res");
        let classes_dir = build_dir.join("classes");
        let native_lib_dir = build_dir.join("lib");
        let dex_dir = build_dir.join("dex");
        for dir in [&gen_dir, &compiled_res_dir, &classes_dir, &native_lib_dir, &dex_dir] {
            fs::create_dir_all(dir)?;
        }

        let cache = StageCache::new(&build_dir)?;

        Ok(Self {
            toolchain,
            project_dir,
            build_dir,
            mode,
            variant,
            on_progress: Box::new(on_progress),
            cache,
            gen_dir,
            compiled_res_dir,
            classes_dir,
            native_lib_dir,
            dex_dir,
            total_warnings: 0,
            total_errors: 0,
            tool_invocations_this_stage: 0,
        })
    }

    fn has_native(&self) -> bool {
        self.project_dir.join("plugin/cpp").exists()
    }
    fn has_rust(&self) -> bool {
        self.project_dir.join("plugin/rust").exists()
    }
    fn has_qml(&self) -> bool {
        self.project_dir.join("plugin/qml").exists()
    }

    /// Runs one external tool invocation as its own complete,
    /// self-announcing unit: prints StageStarted right before
    /// spawning the process (so the exact command about to run is
    /// visible immediately — the same moment a developer would see it
    /// if they'd typed it themselves), then StageFinished once it
    /// exits. This is what makes a stage that shells out multiple
    /// times internally (each ABI in the C++ stage, for instance)
    /// surface as multiple distinct, individually-visible task runs
    /// rather than one opaque "stage" hiding several real commands
    /// inside it — the CLI's task-name renderer keys off `stage_name`
    /// alone, so distinct names naturally become distinct task lines.
    fn run(
        &mut self,
        stage_name: &str,
        program: &Path,
        args: &[String],
        working_dir: Option<&Path>,
    ) -> Result<bool> {
        let working_dir = working_dir.unwrap_or(&self.project_dir).to_path_buf();

        (self.on_progress)(BuildEvent::StageStarted {
            stage: stage_name.to_string(),
            index: 0,
            total: 0,
        });
        self.tool_invocations_this_stage += 1;

        let warnings_before = self.total_warnings;
        let errors_before = self.total_errors;
        let mut warnings = self.total_warnings;
        let mut errors = self.total_errors;
        let on_progress = &mut self.on_progress;
        let ok = run_tool(
            ToolRun {
                stage_name,
                program,
                args,
                working_dir: &working_dir,
            },
            &mut warnings,
            &mut errors,
            |event| on_progress(event),
        )?;
        self.total_warnings = warnings;
        self.total_errors = errors;

        (self.on_progress)(BuildEvent::StageFinished {
            stage: stage_name.to_string(),
            success: ok,
            warning_count: self.total_warnings - warnings_before,
            error_count: self.total_errors - errors_before,
            cached: false,
        });

        Ok(ok)
    }

    fn fail_missing_tool(&mut self, stage: &str, tool_name: &str) -> bool {
        self.total_errors += 1;
        (self.on_progress)(BuildEvent::StageOutput {
            stage: stage.to_string(),
            line: format!("error: required tool not found: {tool_name}. Skipping stage."),
            severity: types::LineSeverity::Error,
        });
        false
    }

    /// Runs the full pipeline. Returns the final `plugin.zip` path on
    /// success, `None` on failure — matching the original's
    /// `build(): File?` return contract.
    pub fn build(&mut self) -> Result<Option<PathBuf>> {
        let has_qml = self.has_qml();
        let has_native = self.has_native();
        let has_rust = self.has_rust();
        let variant = self.variant;

        let stage_defs: Vec<(&'static str, bool, fn(&mut Self) -> Result<StageOutcome>)> = vec![
            ("Resources", true, run_aapt2 as fn(&mut Self) -> Result<StageOutcome>),
            ("Qt/QML", has_qml, run_qt_build),
            ("Native (C++)", has_native, run_clang),
            ("Native (Rust)", has_rust, run_rust_and_clippy),
            ("Kotlin/Java", true, run_kotlinc),
            (
                if variant == BuildVariant::Release { "Dexing (R8)" } else { "Dexing" },
                true,
                run_d8,
            ),
            ("Package & Align", true, run_package),
        ];

        let applicable: Vec<_> = stage_defs.into_iter().filter(|(_, applies, _)| *applies).collect();
        let _total = applicable.len();
        let mut output_file = None;

        for (i, (name, _, action)) in applicable.into_iter().enumerate() {
            let _ = i; // index/total no longer drive display; kept for potential future use
            self.tool_invocations_this_stage = 0;

            let warnings_before = self.total_warnings;
            let errors_before = self.total_errors;

            let outcome = match action(self) {
                Ok(o) => o,
                Err(e) => {
                    (self.on_progress)(BuildEvent::StageOutput {
                        stage: name.to_string(),
                        line: format!("error: stage panicked/errored: {e:#}"),
                        severity: types::LineSeverity::Error,
                    });
                    self.total_errors += 1;
                    StageOutcome::Failed
                }
            };

            let ok = !matches!(outcome, StageOutcome::Failed);
            let cached = matches!(outcome, StageOutcome::Skipped);

            // Only synthesize an announcement here if the stage never
            // called engine.run() itself — a stage that DID invoke a
            // real tool already announced (possibly several times,
            // once per ABI) via run()'s own StageStarted/Finished
            // pair, and re-announcing at this level would print a
            // confusing extra "task" that maps to no actual command.
            if self.tool_invocations_this_stage == 0 {
                (self.on_progress)(BuildEvent::StageStarted {
                    stage: name.to_string(),
                    index: 0,
                    total: 0,
                });
                (self.on_progress)(BuildEvent::StageFinished {
                    stage: name.to_string(),
                    success: ok,
                    warning_count: self.total_warnings - warnings_before,
                    error_count: self.total_errors - errors_before,
                    cached,
                });
            }

            // Nightmare mode: any warning fails the stage even if the
            // tool's own exit code was 0 — enforced uniformly across
            // every language/stage, same as the original.
            let nightmare_warning_failure = self.mode == BuildMode::Nightmare
                && (self.total_warnings - warnings_before) > 0;

            if !ok || nightmare_warning_failure {
                (self.on_progress)(BuildEvent::BuildFinished {
                    success: false,
                    output_file: None,
                    total_warnings: self.total_warnings,
                    total_errors: self.total_errors,
                });
                return Ok(None);
            }

            if let StageOutcome::PackageProduced(file) = outcome {
                output_file = file;
            }
        }

        (self.on_progress)(BuildEvent::BuildFinished {
            success: true,
            output_file: output_file.clone(),
            total_warnings: self.total_warnings,
            total_errors: self.total_errors,
        });
        Ok(output_file)
    }
}

/// Stage 1: Resources. Entirely optional — Youki Shell's plugin.json
/// format has no res/ or XML layout concept (cardBackground is a raw
/// image path). Exists only for developers bringing a traditional
/// Android res/ + XML layout background. A missing res/ dir is a
/// silent no-op, same as the original.
fn run_aapt2(engine: &mut PluginBuildEngine) -> Result<StageOutcome> {
    let res_dir = engine.project_dir.join("plugin/res");
    if !res_dir.exists() {
        return Ok(StageOutcome::Ok);
    }

    let flat_files: Vec<_> = walkdir::WalkDir::new(&res_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    if flat_files.is_empty() {
        return Ok(StageOutcome::Ok);
    }

    let aapt2 = match &engine.toolchain.aapt2 {
        Some(p) => p.clone(),
        None => return Ok(stage_result(engine.fail_missing_tool("Resources", "aapt2"))),
    };

    let compile_ok = engine.run(
        "Resources",
        &aapt2,
        &[
            "compile".into(),
            "--dir".into(),
            res_dir.display().to_string(),
            "-o".into(),
            engine.compiled_res_dir.display().to_string(),
        ],
        None,
    )?;
    if !compile_ok {
        return Ok(StageOutcome::Failed);
    }

    // aapt2 link requires a real AndroidManifest.xml — it cannot parse
    // plugin.json. Generate a minimal valid manifest purely to satisfy
    // aapt2's parser; this file is build-internal only and never
    // appears in the final plugin.zip.
    let manifest = youki_manifest::PluginManifest::load_or_default(&engine.project_dir);
    let generated_manifest = engine.build_dir.join("AndroidManifest.xml");
    fs::write(
        &generated_manifest,
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="{}">
</manifest>
"#,
            manifest.plugin_id
        ),
    )?;

    let android_jar = match &engine.toolchain.android_jar {
        Some(p) => p.clone(),
        None => return Ok(stage_result(engine.fail_missing_tool("Resources", "android.jar"))),
    };

    let flat_res_files: Vec<String> = fs::read_dir(&engine.compiled_res_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|ext| ext == "flat").unwrap_or(false))
        .map(|p| p.display().to_string())
        .collect();

    let mut args = vec![
        "link".to_string(),
        "-o".into(),
        engine.build_dir.join("resources.apk").display().to_string(),
        "-I".into(),
        android_jar.display().to_string(),
        "--manifest".into(),
        generated_manifest.display().to_string(),
        "--java".into(),
        engine.gen_dir.display().to_string(),
        "--auto-add-overlay".into(),
    ];
    args.extend(flat_res_files);

    let ok = engine.run("Resources", &aapt2, &args, None)?;
    Ok(stage_result(ok))
}

/// Stage 2: Qt/QML. Unlike every other stage, does NOT call
/// moc/rcc/clang directly — delegates to the developer's own
/// Qt-for-Android CMake installation, since Qt's CMake "finalize" step
/// (AndroidManifest fragments, deployment metadata, linking rules) is
/// load-bearing and not something a hand-rolled clang++ call can
/// replicate.
fn run_qt_build(engine: &mut PluginBuildEngine) -> Result<StageOutcome> {
    let qml_dir = engine.project_dir.join("plugin/qml");
    let cmake_lists = qml_dir.join("CMakeLists.txt");

    let cmake = match &engine.toolchain.cmake {
        Some(p) => p.clone(),
        None => {
            return Ok(stage_result(engine.fail_missing_tool(
                "Qt/QML",
                "cmake (with a Qt-for-Android toolchain installed)",
            )))
        }
    };

    if !cmake_lists.exists() {
        (engine.on_progress)(BuildEvent::StageOutput {
            stage: "Qt/QML".into(),
            line: "warning: plugin/qml/ exists but has no CMakeLists.txt — skipping Qt build. \
                A Qt/QML plugin module needs its own CMakeLists.txt using Qt's official CMake \
                macros (qt_add_executable/qt_add_qml_module) — see Qt for Android docs."
                .into(),
            severity: types::LineSeverity::Warning,
        });
        engine.total_warnings += 1;
        return Ok(StageOutcome::Ok);
    }

    let qt_toolchain = match &engine.toolchain.qt_cmake_toolchain_file {
        Some(p) => p.clone(),
        None => {
            return Ok(stage_result(engine.fail_missing_tool(
                "Qt/QML",
                "Qt-for-Android CMake toolchain file (qt.toolchain.cmake)",
            )))
        }
    };

    let build_type = if engine.variant == BuildVariant::Release { "Release" } else { "Debug" };

    // SECURITY/CORRECTNESS FIX: this stage used to configure+build
    // exactly ONCE with a hardcoded -DANDROID_ABI=arm64-v8a, then copy
    // that single arm64-v8a binary into every ABI directory
    // requirements.abis asked for. Two real bugs followed from that:
    // (1) a plugin declaring e.g. abis: ["armeabi-v7a", "x86_64"] (no
    // arm64-v8a at all) would get an arm64-v8a binary mislabeled and
    // shipped under armeabi-v7a/ and x86_64/ — which crashes at load
    // time on the real device, since the instruction set doesn't
    // match the label; (2) a plugin with BOTH plugin/cpp and
    // plugin/qml building the same ABI could have the two stages'
    // outputs collide in the exact same lib/<abi>/ directory if
    // filenames matched, with whichever stage ran second silently
    // overwriting the first's file with no warning at all.
    //
    // Fixed by actually reconfiguring and rebuilding once per ABI in
    // requirements.abis, each in its OWN cmake build subdirectory
    // (qt_cmake_build/<abi>/) so one ABI's CMakeCache.txt/object files
    // can never be reused or bleed into another ABI's build.
    let manifest = youki_manifest::PluginManifest::load_or_default(&engine.project_dir);
    let target_abis = manifest.requirements.target_abis();
    if target_abis.is_empty() {
        (engine.on_progress)(BuildEvent::StageOutput {
            stage: "Qt/QML".into(),
            line: "error: requirements.abis resolved to zero recognized ABIs — nothing to build"
                .into(),
            severity: types::LineSeverity::Error,
        });
        engine.total_errors += 1;
        return Ok(StageOutcome::Failed);
    }

    for abi in &target_abis {
        let cmake_build_dir = engine.build_dir.join("qt_cmake_build").join(abi.as_str());
        fs::create_dir_all(&cmake_build_dir)?;
        let stage_label = format!("Qt/QML [{}]", abi.as_str());

        let configure_ok = engine.run(
            &stage_label,
            &cmake,
            &[
                "-S".into(),
                qml_dir.display().to_string(),
                "-B".into(),
                cmake_build_dir.display().to_string(),
                format!("-DCMAKE_TOOLCHAIN_FILE={}", qt_toolchain.display()),
                format!("-DANDROID_ABI={}", abi.as_str()),
                format!("-DCMAKE_BUILD_TYPE={build_type}"),
            ],
            None,
        )?;
        if !configure_ok {
            return Ok(StageOutcome::Failed);
        }

        let build_ok = engine.run(
            &stage_label,
            &cmake,
            &["--build".into(), cmake_build_dir.display().to_string()],
            None,
        )?;
        if !build_ok {
            return Ok(StageOutcome::Failed);
        }

        // This ABI's own dedicated build directory guarantees these
        // .so files are genuinely this ABI's output.
        let qt_output_libs: Vec<_> = walkdir::WalkDir::new(&cmake_build_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "so").unwrap_or(false))
            .map(|e| e.path().to_path_buf())
            .collect();

        if qt_output_libs.is_empty() {
            (engine.on_progress)(BuildEvent::StageOutput {
                stage: stage_label.clone(),
                line: "warning: Qt/CMake build finished but produced no .so files for this ABI"
                    .into(),
                severity: types::LineSeverity::Warning,
            });
            engine.total_warnings += 1;
        }

        let abi_lib_dir = engine.native_lib_dir.join(abi.as_str());
        fs::create_dir_all(&abi_lib_dir)?;
        for lib in qt_output_libs {
            if let Some(file_name) = lib.file_name() {
                fs::copy(&lib, abi_lib_dir.join(file_name))?;
            }
        }
    }

    Ok(StageOutcome::Ok)
}

/// Stage 3: Native C++. Plain project C++ sources only — Qt/QML has
/// its own stage above. Nightmare adds sanitizers, the one place it
/// catches actual memory bugs rather than just stricter warnings.
///
/// Compiles once PER ABI listed in plugin.json's `requirements.abis`
/// (default: arm64-v8a only). Each ABI's clang invocation is otherwise
/// identical except for `--target=` and its output subdirectory —
/// this is exactly the "device only has the .so for its own CPU
/// architecture" mechanism Android's own jniLibs/lib folder convention
/// relies on (see `Android ABIs` in the NDK docs: one `lib/<abi>/`
/// subfolder per architecture inside the final package).
fn run_clang(engine: &mut PluginBuildEngine) -> Result<StageOutcome> {
    let clangxx = match &engine.toolchain.clangxx {
        Some(p) => p.clone(),
        None => return Ok(stage_result(engine.fail_missing_tool("Native (C++)", "clang++ (Android NDK)"))),
    };

    let cpp_dir = engine.project_dir.join("plugin/cpp");
    let cpp_source_paths: Vec<PathBuf> = walkdir::WalkDir::new(&cpp_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "cpp" || ext == "cc")
                .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect();
    let cpp_sources: Vec<String> = cpp_source_paths.iter().map(|p| p.display().to_string()).collect();

    if cpp_sources.is_empty() {
        return Ok(StageOutcome::Ok);
    }

    let strict_flags: Vec<String> = match engine.mode {
        BuildMode::Peaceful => vec!["-w".into()],
        BuildMode::Dictator => vec!["-Wall".into(), "-Wextra".into()],
        BuildMode::Nightmare => vec![
            "-Wall".into(),
            "-Wextra".into(),
            "-Werror".into(),
            "-Wpedantic".into(),
            "-fsanitize=address,undefined".into(),
            "-fno-omit-frame-pointer".into(),
        ],
    };

    let manifest = youki_manifest::PluginManifest::load_or_default(&engine.project_dir);
    let min_sdk = manifest.requirements.min_sdk;
    let target_abis = manifest.requirements.target_abis();
    let mut any_stage_ran = false;

    for abi in target_abis {
        let abi_out_dir = engine.native_lib_dir.join(abi.as_str());
        fs::create_dir_all(&abi_out_dir)?;
        let output_so = abi_out_dir.join("libplugin_native.so");

        // BUILD CACHE: per-ABI, since each ABI is a genuinely separate
        // compile with a different --target= and output file. A cache
        // hit on arm64-v8a must not be assumed to also cover
        // armeabi-v7a — they're fingerprinted independently.
        let stage_key = format!("clang_{}", abi.as_str());
        let fingerprint = cache::StageCache::fingerprint(
            &cpp_source_paths,
            &[
                &clangxx.display().to_string(),
                abi.clang_target_prefix(),
                &min_sdk.to_string(),
                &format!("{:?}", engine.mode),
            ],
        );
        if engine
            .cache
            .is_up_to_date(&stage_key, &fingerprint, &[output_so.clone()])
        {
            // Announce this ABI's own individual cache hit — without
            // this, a partial cache state (arm64-v8a cached, but
            // armeabi-v7a needs rebuilding) would show only the
            // rebuilt ABI's task and leave the cached one invisible.
            let stage_label = format!("Native (C++) [{}]", abi.as_str());
            engine.tool_invocations_this_stage += 1;
            (engine.on_progress)(BuildEvent::StageStarted {
                stage: stage_label.clone(),
                index: 0,
                total: 0,
            });
            (engine.on_progress)(BuildEvent::StageFinished {
                stage: stage_label,
                success: true,
                warning_count: 0,
                error_count: 0,
                cached: true,
            });
            continue;
        }

        let mut args = vec![
            format!("--target={}{min_sdk}", abi.clang_target_prefix()),
            "-shared".into(),
            "-fPIC".into(),
            "-O2".into(),
        ];
        args.extend(strict_flags.clone());
        args.extend(cpp_sources.clone());
        args.push("-o".into());
        args.push(output_so.display().to_string());
        any_stage_ran = true;

        let stage_label = format!("Native (C++) [{}]", abi.as_str());
        let ok = engine.run(&stage_label, &clangxx, &args, None)?;
        if !ok {
            return Ok(StageOutcome::Failed);
        }
        engine.cache.record(&stage_key, &fingerprint)?;
    }

    Ok(if any_stage_ran { StageOutcome::Ok } else { StageOutcome::Skipped })
}

/// Stage 4: Native Rust. Peaceful == Dictator (rustc is already strict
/// by default). Nightmare adds `cargo clippy` plus `-D warnings`.
fn run_rust_and_clippy(engine: &mut PluginBuildEngine) -> Result<StageOutcome> {
    let cargo_ndk = match &engine.toolchain.cargo_ndk {
        Some(p) => p.clone(),
        None => return Ok(stage_result(engine.fail_missing_tool("Native (Rust)", "cargo-ndk"))),
    };

    let rust_project_dir = engine.project_dir.join("plugin/rust");
    if !rust_project_dir.join("Cargo.toml").exists() {
        return Ok(StageOutcome::Ok);
    }

    if engine.mode == BuildMode::Nightmare {
        let clippy = match &engine.toolchain.cargo_clippy {
            Some(p) => p.clone(),
            None => return Ok(stage_result(engine.fail_missing_tool("Native (Rust)", "cargo-clippy"))),
        };
        let clippy_ok = engine.run(
            "Native (Rust)",
            &clippy,
            &[
                "--all-targets".into(),
                "--".into(),
                "-D".into(),
                "warnings".into(),
                "-D".into(),
                "clippy::all".into(),
            ],
            Some(&rust_project_dir),
        )?;
        if !clippy_ok {
            return Ok(StageOutcome::Failed);
        }
    }

    let deny_warnings = engine.mode == BuildMode::Nightmare;
    let manifest = youki_manifest::PluginManifest::load_or_default(&engine.project_dir);
    let target_abis = manifest.requirements.target_abis();

    // BUILD CACHE: fingerprints Cargo.toml, Cargo.lock (dependency
    // version changes must invalidate the cache even with unchanged
    // source), and every .rs file under the crate — plus the set of
    // target ABIs and cargo-ndk's own path. Expected outputs are each
    // ABI's .so landing in native_lib_dir/<abi>/ — cargo-ndk names the
    // file after the crate's [lib] name, which this engine doesn't
    // control, so instead of guessing the filename this checks that
    // each ABI's output directory is non-empty, which is what
    // packaging actually requires downstream.
    let mut rust_input_files: Vec<PathBuf> = walkdir::WalkDir::new(&rust_project_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "rs").unwrap_or(false))
        .map(|e| e.path().to_path_buf())
        .collect();
    let cargo_toml = rust_project_dir.join("Cargo.toml");
    let cargo_lock = rust_project_dir.join("Cargo.lock");
    if cargo_toml.exists() {
        rust_input_files.push(cargo_toml);
    }
    if cargo_lock.exists() {
        rust_input_files.push(cargo_lock);
    }

    let abi_names: Vec<&str> = target_abis.iter().map(|a| a.as_str()).collect();
    let fingerprint = cache::StageCache::fingerprint(
        &rust_input_files,
        &[
            &cargo_ndk.display().to_string(),
            &abi_names.join(","),
            &deny_warnings.to_string(),
        ],
    );

    let all_abi_dirs_populated = !target_abis.is_empty()
        && target_abis.iter().all(|abi| {
            let dir = engine.native_lib_dir.join(abi.as_str());
            fs::read_dir(&dir)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false)
        });

    if all_abi_dirs_populated && engine.cache.is_up_to_date("cargo_ndk", &fingerprint, &[]) {
        return Ok(StageOutcome::Skipped);
    }

    // cargo-ndk accepts multiple `-t <abi>` flags in a single
    // invocation and cross-compiles for all of them in one build,
    // writing each ABI's .so into its own native_lib_dir/<abi>/
    // subfolder automatically — no need to loop/re-invoke per ABI the
    // way plain clang++ (Stage 3) does.
    let mut args = vec!["-o".to_string(), engine.native_lib_dir.display().to_string()];
    for abi in &target_abis {
        args.push("-t".to_string());
        args.push(abi.as_str().to_string());
    }
    args.push("build".into());
    args.push("--release".into());
    if deny_warnings {
        args.extend(["--".to_string(), "-D".into(), "warnings".into()]);
    }

    let ok = engine.run("Native (Rust)", &cargo_ndk, &args, Some(&rust_project_dir))?;
    if ok {
        engine.cache.record("cargo_ndk", &fingerprint)?;
    }
    Ok(stage_result(ok))
}

/// Stage 5: Kotlin/Java — runs after Resources (may reference R.java).
///
/// Directory naming convention (deliberately strict, no fallback):
/// `plugin/kotlin/` is for Kotlin-only sources. `plugin/java/` accepts
/// BOTH Java and Kotlin files together — a project that names its
/// source folder "java" is signaling it may contain interop code, so
/// kotlinc (which compiles .java files referenced from .kt just fine)
/// is pointed at both extensions there. A project using "kotlin/" is
/// asserting no Java in play; a stray .java file dropped there is not
/// specially discovered — it simply won't be picked up in that
/// directory, which is intentional: the directory name is meant to be
/// a real declaration of intent, not just a convention that happens
/// to work either way.
fn run_kotlinc(engine: &mut PluginBuildEngine) -> Result<StageOutcome> {
    let src_dir_paths: Vec<PathBuf> = ["plugin/kotlin", "plugin/java"]
        .iter()
        .map(|d| engine.project_dir.join(d))
        .filter(|p| p.exists())
        .collect();
    let src_dirs: Vec<String> = src_dir_paths.iter().map(|p| p.display().to_string()).collect();

    let kotlinc = match &engine.toolchain.kotlinc {
        Some(p) => p.clone(),
        None => return Ok(stage_result(engine.fail_missing_tool("Kotlin/Java", "kotlinc"))),
    };
    let android_jar = match &engine.toolchain.android_jar {
        Some(p) => p.clone(),
        None => return Ok(stage_result(engine.fail_missing_tool("Kotlin/Java", "android.jar"))),
    };

    let strict_args: Vec<String> = match engine.mode {
        BuildMode::Peaceful => vec!["-nowarn".into()],
        BuildMode::Dictator => vec!["-Xextended-compiler-checks".into()],
        BuildMode::Nightmare => vec!["-Werror".into(), "-Xextended-compiler-checks".into()],
    };

    // BUILD CACHE: fingerprint every .kt/.java source file's contents,
    // plus context that changes kotlinc's output even with identical
    // sources — the mode's strictness flags (peaceful vs nightmare
    // produces different .class output/behavior) and the kotlinc/
    // android.jar paths themselves (a toolchain upgrade must
    // invalidate a stale cache, not just source edits). If the
    // fingerprint matches AND classes_dir still has at least one
    // .class file from last time, skip recompiling entirely.
    let source_files: Vec<PathBuf> = src_dir_paths
        .iter()
        .flat_map(|dir| {
            walkdir::WalkDir::new(dir)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "kt" || ext == "java")
                        .unwrap_or(false)
                })
                .map(|e| e.path().to_path_buf())
        })
        .collect();

    let fingerprint = cache::StageCache::fingerprint(
        &source_files,
        &[
            &kotlinc.display().to_string(),
            &android_jar.display().to_string(),
            &format!("{:?}", engine.mode),
        ],
    );

    let has_prior_output = fs::read_dir(&engine.classes_dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    let expected_outputs: Vec<PathBuf> = if has_prior_output {
        vec![engine.classes_dir.clone()]
    } else {
        vec![]
    };

    if !expected_outputs.is_empty()
        && engine
            .cache
            .is_up_to_date("kotlinc", &fingerprint, &expected_outputs)
    {
        return Ok(StageOutcome::Skipped);
    }

    let mut args = src_dirs;
    args.push(engine.gen_dir.display().to_string());
    args.push("-cp".into());
    args.push(android_jar.display().to_string());
    args.push("-d".into());
    args.push(engine.classes_dir.display().to_string());
    args.extend(strict_args);

    let ok = engine.run("Kotlin/Java", &kotlinc, &args, None)?;
    if ok {
        engine.cache.record("kotlinc", &fingerprint)?;
    }
    Ok(stage_result(ok))
}

/// Stage 6: Dexing. DEBUG uses d8 directly (fast, no shrinking).
/// RELEASE uses r8 instead: same class-to-dex conversion, plus
/// shrinking + obfuscation. The one point where Debug/Release diverge
/// in *which binary runs*, not just which flags get passed.
fn run_d8(engine: &mut PluginBuildEngine) -> Result<StageOutcome> {
    let class_file_paths: Vec<PathBuf> = walkdir::WalkDir::new(&engine.classes_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "class").unwrap_or(false))
        .map(|e| e.path().to_path_buf())
        .collect();
    let class_files: Vec<String> = class_file_paths.iter().map(|p| p.display().to_string()).collect();

    if class_files.is_empty() {
        return Ok(StageOutcome::Ok);
    }

    let android_jar = match &engine.toolchain.android_jar {
        Some(p) => p.clone(),
        None => return Ok(stage_result(engine.fail_missing_tool("Dexing", "android.jar"))),
    };

    // --min-api MUST come from the plugin's own minSdk, not a hardcoded
    // constant — d8/r8 use this to decide which language/library
    // features need desugaring for older devices. A plugin declaring
    // minSdk 26 gets different (and correct) desugaring than one
    // declaring minSdk 21 would.
    let manifest = youki_manifest::PluginManifest::load_or_default(&engine.project_dir);
    let min_api = manifest.requirements.min_sdk.to_string();
    let output_dex = engine.dex_dir.join("classes.dex");

    match engine.variant {
        BuildVariant::Debug => {
            let d8 = match &engine.toolchain.d8 {
                Some(p) => p.clone(),
                None => return Ok(stage_result(engine.fail_missing_tool("Dexing", "d8"))),
            };

            // BUILD CACHE: fingerprints the actual .class file bytes
            // (not the Kotlin/Java cache's own hash — a class file's
            // bytes are the true dependency here, so this stage stays
            // correct even if it were ever invoked independently of
            // run_kotlinc's cache state) plus minSdk and the d8 binary
            // path itself.
            let fingerprint = cache::StageCache::fingerprint(
                &class_file_paths,
                &[&d8.display().to_string(), &min_api, "debug"],
            );
            if engine.cache.is_up_to_date("d8", &fingerprint, &[output_dex.clone()]) {
                return Ok(StageOutcome::Skipped);
            }

            let mut args = vec![
                "--debug".to_string(),
                "--min-api".into(),
                min_api,
                "--lib".into(),
                android_jar.display().to_string(),
                "--output".into(),
                engine.dex_dir.display().to_string(),
            ];
            args.extend(class_files);
            let ok = engine.run("Dexing", &d8, &args, None)?;
            if ok {
                engine.cache.record("d8", &fingerprint)?;
            }
            Ok(stage_result(ok))
        }
        BuildVariant::Release => {
            let r8 = match &engine.toolchain.r8 {
                Some(p) => p.clone(),
                None => return Ok(stage_result(engine.fail_missing_tool("Dexing (R8)", "r8"))),
            };
            let proguard_rules = engine.project_dir.join("plugin/proguard-rules.pro");

            // Release also fingerprints proguard-rules.pro's contents
            // — a rules-only edit (no source change at all) must still
            // invalidate the cache, since r8's shrinking/obfuscation
            // output depends on it directly.
            let mut fingerprint_inputs = class_file_paths.clone();
            if proguard_rules.exists() {
                fingerprint_inputs.push(proguard_rules.clone());
            }
            let fingerprint = cache::StageCache::fingerprint(
                &fingerprint_inputs,
                &[&r8.display().to_string(), &min_api, "release"],
            );
            if engine.cache.is_up_to_date("r8", &fingerprint, &[output_dex.clone()]) {
                return Ok(StageOutcome::Skipped);
            }

            let mut args = vec![
                "--release".to_string(),
                "--min-api".into(),
                min_api,
                "--lib".into(),
                android_jar.display().to_string(),
                "--output".into(),
                engine.dex_dir.display().to_string(),
            ];
            if proguard_rules.exists() {
                args.push("--pg-conf".into());
                args.push(proguard_rules.display().to_string());
            }
            args.extend(class_files);
            let ok = engine.run("Dexing (R8)", &r8, &args, None)?;
            if ok {
                engine.cache.record("r8", &fingerprint)?;
            }
            Ok(stage_result(ok))
        }
    }
}

/// Stage 7: Package & Align — bundles everything into the final
/// plugin.zip, then runs zipalign. Preserves the original's exact
/// bundling order and its dexPath-rename behavior (d8/r8 always name
/// their own output "classes.dex"; the manifest's `entry.dexPath` does
/// not have to match, so we rename before zipping if needed).
fn run_package(engine: &mut PluginBuildEngine) -> Result<StageOutcome> {
    let unaligned = engine.build_dir.join("unaligned_plugin.zip");
    let _ = fs::remove_file(&unaligned);

    zip_add(&unaligned, &engine.project_dir, &["plugin.json"], false)?;

    let dex_file = engine.dex_dir.join("classes.dex");
    if dex_file.exists() {
        let manifest = youki_manifest::PluginManifest::load_or_default(&engine.project_dir);
        let requested_dex_path = manifest.dex_path();
        let requested = Path::new(&requested_dex_path);
        let target_name = requested
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "classes.dex".to_string());
        let target_dir = requested.parent().filter(|p| !p.as_os_str().is_empty());

        let file_to_zip = if target_name != "classes.dex" {
            let renamed = engine.dex_dir.join(&target_name);
            fs::copy(&dex_file, &renamed)?;
            renamed
        } else {
            dex_file.clone()
        };

        match target_dir {
            None => {
                zip_add_junk_paths(&unaligned, &engine.build_dir, &[file_to_zip.display().to_string()])?;
            }
            Some(subdir) => {
                // dexPath includes a subdirectory — stage it at that
                // relative path before zipping, since junk-paths mode
                // would otherwise flatten it to the root.
                let staging_dir = engine.build_dir.join("dex_staging").join(subdir);
                fs::create_dir_all(&staging_dir)?;
                fs::copy(&file_to_zip, staging_dir.join(&target_name))?;
                zip_add(
                    &unaligned,
                    &engine.build_dir.join("dex_staging"),
                    &[subdir.display().to_string().as_str()],
                    true,
                )?;
            }
        }
    }

    if dir_has_files(&engine.native_lib_dir) {
        zip_add(&unaligned, &engine.build_dir, &["lib"], true)?;
    }

    // Merge build-generated assets (e.g. Qt/QML's qt_resources.rcc)
    // with the project's own hand-authored assets, if both exist —
    // both belong under assets/ in the final package.
    //
    // NOTE: base_dir here is `project_dir.join("plugin")`, not
    // `project_dir` itself — the source layout moved to plugin/assets,
    // plugin/graphics, etc, but the *zip's internal* path must stay
    // exactly "assets/", "graphics/", "webview/" at the archive root
    // (unchanged), since that's YoukiShell's own documented bundle
    // format and has nothing to do with this tool's source layout.
    let plugin_src_dir = engine.project_dir.join("plugin");

    let assets_out_dir = engine.build_dir.join("assets");
    if dir_has_files(&assets_out_dir) {
        zip_add(&unaligned, &engine.build_dir, &["assets"], true)?;
    }
    let project_assets_dir = plugin_src_dir.join("assets");
    if dir_has_files(&project_assets_dir) {
        zip_add(&unaligned, &plugin_src_dir, &["assets"], true)?;
    }

    // graphics/ and webview/ are real, documented parts of Youki
    // Shell's bundle format — copied as-is, no processing needed.
    let graphics_dir = plugin_src_dir.join("graphics");
    if dir_has_files(&graphics_dir) {
        zip_add(&unaligned, &plugin_src_dir, &["graphics"], true)?;
    }
    let webview_dir = plugin_src_dir.join("webview");
    if dir_has_files(&webview_dir) {
        zip_add(&unaligned, &plugin_src_dir, &["webview"], true)?;
    }

    // bin/ — native.exec ELF helper binaries. Youki Shell's guide is
    // explicit these need the execute bit set at zip creation time.
    // Set explicitly rather than relying on the source file's
    // permissions, since a binary copied in by some other build step
    // might land without +x.
    let bin_dir = plugin_src_dir.join("bin");
    if dir_has_files(&bin_dir) {
        set_executable_recursive(&bin_dir)?;
        zip_add(&unaligned, &plugin_src_dir, &["bin"], true)?;
    }

    // SECURITY/CORRECTNESS FIX: this used to write the final package to
    // `project_dir/output/plugin.zip` — i.e. INSIDE the same directory
    // as the project's own source (plugin.json, src/, assets/,
    // graphics/...). That breaks the separation every other stage in
    // this engine relies on (all intermediate artifacts live under
    // build_dir, well away from anything zip_add ever scans). Concrete
    // failure modes that followed from the old location:
    //   - Re-running `youki build` leaves a stale plugin.zip sitting in
    //     the source tree between builds; a build that fails partway
    //     through could leave a developer inspecting an old .zip while
    //     believing it's fresh.
    //   - Anything that packages/copies "the whole project directory"
    //     (a naive `zip -r`, `tar`, `cp -r` for backup, an unwary `git
    //     add .` without a correct .gitignore) sweeps up a large
    //     binary build artifact right along with source files.
    //   - It muddies the one invariant this engine otherwise holds
    //     everywhere else: build_dir is the sole location for anything
    //     this tool produces; project_dir is read-only input.
    // Fixed by writing the final package under build_dir instead, and
    // only copying it out to the project's output/ as the very last
    // step — so a failed or stale build never leaves an ambiguous
    // artifact sitting in the source tree.
    let build_output_dir = engine.build_dir.join("output");
    fs::create_dir_all(&build_output_dir)?;
    // Youki Shell's README documents the bundle as a plain .zip
    // ("myplugin.zip"), not a custom .plugin extension — matched
    // exactly rather than inventing a different one.
    let build_final_file = build_output_dir.join("plugin.zip");

    let zipalign = match &engine.toolchain.zipalign {
        Some(p) => p.clone(),
        None => return Ok(stage_result(engine.fail_missing_tool("Package & Align", "zipalign"))),
    };

    let aligned = engine.run(
        "Package & Align",
        &zipalign,
        &[
            "-v".into(),
            "-f".into(),
            "4".into(),
            unaligned.display().to_string(),
            build_final_file.display().to_string(),
        ],
        None,
    )?;

    if !aligned {
        return Ok(StageOutcome::Failed);
    }

    // Only now, once zipalign has genuinely succeeded, publish the
    // result to project_dir/output/ for the developer to find — a
    // failed build never leaves anything (stale or otherwise) there.
    let published_output_dir = engine.project_dir.join("output");
    fs::create_dir_all(&published_output_dir)?;
    let published_file = published_output_dir.join("plugin.zip");
    fs::copy(&build_final_file, &published_file)?;

    Ok(StageOutcome::PackageProduced(Some(published_file)))
}

fn stage_result(ok: bool) -> StageOutcome {
    if ok {
        StageOutcome::Ok
    } else {
        StageOutcome::Failed
    }
}

fn dir_has_files(dir: &Path) -> bool {
    dir.exists()
        && fs::read_dir(dir)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
}

fn set_executable_recursive(dir: &Path) -> Result<()> {
    #[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let mut perms = fs::metadata(entry.path())?.permissions();
            let mode = perms.mode();
            perms.set_mode(mode | 0o111);
            fs::set_permissions(entry.path(), perms)?;
        }
    }
    Ok(())
}

/// Adds `entries` (files or directories, relative to `base_dir`) into
/// `zip_path`, preserving relative paths — equivalent to
/// `zip -r <zip_path> <entries...>` run with `base_dir` as cwd.
fn zip_add(zip_path: &Path, base_dir: &Path, entries: &[&str], recursive: bool) -> Result<()> {
    let _ = recursive; // recursion is inherent to walking directories below
    let mut existing_files = load_existing_zip_entries(zip_path)?;

    // ZipWriter::new_append needs read+write (it seeks back to read
    // the existing central directory before appending); write-only
    // fails with EBADF on the first read seek.
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(zip_path)?;
    let mut writer = if !existing_files.is_empty() {
        zip::ZipWriter::new_append(file)?
    } else {
        zip::ZipWriter::new(file)
    };

    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    for entry in entries {
        let full_path = base_dir.join(entry);
        if full_path.is_dir() {
            for walked in walkdir::WalkDir::new(&full_path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
            {
                let rel = walked.path().strip_prefix(base_dir)?;
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if existing_files.insert(rel_str.clone()) {
                    let data = fs::read(walked.path())?;
                    writer.start_file(rel_str, options)?;
                    std::io::Write::write_all(&mut writer, &data)?;
                }
            }
        } else if full_path.is_file() {
            let rel_str = entry.replace('\\', "/");
            if existing_files.insert(rel_str.clone()) {
                let data = fs::read(&full_path)?;
                writer.start_file(rel_str, options)?;
                std::io::Write::write_all(&mut writer, &data)?;
            }
        }
    }

    writer.finish()?;
    Ok(())
}

/// Adds files by basename only, ignoring their original directory
/// structure — equivalent to `zip -j`.
fn zip_add_junk_paths(zip_path: &Path, _base_dir: &Path, file_paths: &[String]) -> Result<()> {
    let mut existing_files = load_existing_zip_entries(zip_path)?;

    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(zip_path)?;
    let mut writer = if !existing_files.is_empty() {
        zip::ZipWriter::new_append(file)?
    } else {
        zip::ZipWriter::new(file)
    };

    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    for path_str in file_paths {
        let path = Path::new(path_str);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.clone());
        if existing_files.insert(name.clone()) {
            let data = fs::read(path)?;
            writer.start_file(name, options)?;
            std::io::Write::write_all(&mut writer, &data)?;
        }
    }

    writer.finish()?;
    Ok(())
}

fn load_existing_zip_entries(zip_path: &Path) -> Result<std::collections::HashSet<String>> {
    let mut set = std::collections::HashSet::new();
    if zip_path.exists() {
        if let Ok(file) = fs::File::open(zip_path) {
            if let Ok(mut archive) = zip::ZipArchive::new(file) {
                for i in 0..archive.len() {
                    if let Ok(entry) = archive.by_index(i) {
                        set.insert(entry.name().to_string());
                    }
                }
            }
        }
    }
    Ok(set)
}
