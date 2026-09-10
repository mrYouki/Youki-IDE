use anyhow::Result;
use clap::Args;
use console::{style, Term};
use std::io::{self, Write as _};
use std::path::PathBuf;
use youki_build_engine::types::{BuildEvent, BuildMode, BuildVariant, LineSeverity};
use youki_build_engine::PluginBuildEngine;
use youki_sdk_manager::ensure::{install_missing, missing_components};
use youki_sdk_manager::SdkManager;
use youki_toolchain::ToolchainPaths;

#[derive(Args)]
pub struct BuildArgs {
    /// Project directory (defaults to current directory)
    #[arg(long, default_value = ".")]
    pub path: PathBuf,

    /// Build variant
    #[arg(long, value_enum, default_value = "debug")]
    pub variant: VariantArg,

    /// Strictness mode
    #[arg(long, value_enum, default_value = "peaceful")]
    pub mode: ModeArg,

    /// Override ANDROID_HOME for this build
    #[arg(long)]
    pub android_home: Option<PathBuf>,

    /// Override ANDROID_NDK_HOME for this build
    #[arg(long)]
    pub ndk_home: Option<PathBuf>,

    /// Automatically download any missing SDK/NDK components instead
    /// of asking for confirmation first. Useful for CI or scripted
    /// builds where nothing can respond to a terminal prompt.
    #[arg(long)]
    pub yes: bool,

    /// Never download anything automatically, even interactively —
    /// just fail with a clear message if something required is
    /// missing. Overrides --yes if both are somehow passed.
    #[arg(long)]
    pub no_auto_install: bool,

    /// Ignore the build cache and force every stage to rerun, even if
    /// its inputs haven't changed since the last successful build.
    /// Does not delete build/ itself (compiled output from stages
    /// that DO end up identical is simply overwritten in place) — use
    /// this to rule out a stale-cache suspicion without a full clean.
    #[arg(long)]
    pub no_cache: bool,
}

#[derive(Clone, clap::ValueEnum)]
pub enum VariantArg {
    Debug,
    Release,
}

#[derive(Clone, clap::ValueEnum)]
pub enum ModeArg {
    Peaceful,
    Dictator,
    Nightmare,
}

pub fn run(args: BuildArgs) -> Result<()> {
    let term = Term::stdout();
    let project_dir = args.path.canonicalize().unwrap_or(args.path.clone());
    let build_dir = project_dir.join("build");

    if args.no_cache {
        let _ = std::fs::remove_dir_all(build_dir.join(".cache"));
    }

    // --- Step 1: what does this project actually need? ---
    // Reads plugin.json's "requirements" block (compileSdk,
    // buildToolsVersion, ndkVersion), falling back to sane defaults
    // (API 34 / build-tools 34.0.0 / no NDK) if the field is absent —
    // this is what makes the auto-detect step below work even for
    // plugin.json files written before "requirements" existed.
    let manifest = youki_manifest::PluginManifest::load_or_default(&project_dir);
    let needs_ndk = project_dir.join("plugin/cpp").exists()
        || project_dir.join("plugin/rust").exists();

    let android_home = args
        .android_home
        .clone()
        .unwrap_or_else(SdkManager::default_location);
    let sdk_mgr = SdkManager::new(&android_home);

    // --- Step 2: auto-detect what's already installed ---
    let missing = missing_components(&sdk_mgr, &manifest.requirements, needs_ndk);

    if !missing.is_empty() {
        term.write_line(&format!(
            "{}",
            style(format!(
                "This project needs {} SDK component(s) that aren't installed yet at {}:",
                missing.len(),
                android_home.display()
            ))
            .yellow()
        ))?;
        for m in &missing {
            term.write_line(&format!("  - {m}"))?;
        }

        if args.no_auto_install {
            anyhow::bail!(
                "missing SDK components and --no-auto-install was passed; run `youki sdk install-*` manually"
            );
        }

        // --- Step 3: install automatically, or ask first ---
        let should_install = args.yes || confirm("\nDownload and install these now? [Y/n] ")?;
        if !should_install {
            anyhow::bail!("required SDK components are missing; re-run with --yes to auto-install");
        }

        term.write_line("")?;
        let mut last_component_label = String::new();
        install_missing(&sdk_mgr, &missing, |component, done, total| {
            let label = component.to_string();
            if label != last_component_label {
                let _ = term.write_line(&format!("Installing {label}..."));
                last_component_label = label;
            }
            if total > 0 && done == total {
                let _ = term.write_line(&format!("  {} done", style("✓").green()));
            }
        })?;
        term.write_line(&format!("{}\n", style("All required components installed.").green()))?;
    }

    // --- Step 4: build, same as before, now guaranteed to find the
    // tools it needs (barring compilers like kotlinc/clang that are
    // system packages, not part of the Android SDK itself) ---
    let toolchain = ToolchainPaths::discover(Some(&android_home), args.ndk_home.as_deref());

    let tool_gaps = toolchain.missing_for_kotlin();
    if !tool_gaps.is_empty() {
        term.write_line(&format!("{}", style("Missing required tools:").red().bold()))?;
        for tool in &tool_gaps {
            term.write_line(&format!("  - {} ({})", tool.name, tool.hint))?;
        }
        term.write_line("")?;
        term.write_line("Run `youki doctor` for a full report.")?;
        anyhow::bail!("missing required tools");
    }

    let mode = match args.mode {
        ModeArg::Peaceful => BuildMode::Peaceful,
        ModeArg::Dictator => BuildMode::Dictator,
        ModeArg::Nightmare => BuildMode::Nightmare,
    };
    let variant = match args.variant {
        VariantArg::Debug => BuildVariant::Debug,
        VariantArg::Release => BuildVariant::Release,
    };
    // Matches Gradle's own PascalCase variant naming inside task names
    // (":app:compileDebugKotlin" / ":app:compileReleaseKotlin").
    let variant_label = match args.variant {
        VariantArg::Debug => "Debug",
        VariantArg::Release => "Release",
    };
    let plugin_id = manifest.plugin_id.clone();

    let term_for_events = term.clone();
    let mut engine = PluginBuildEngine::new(
        &toolchain,
        &project_dir,
        &build_dir,
        mode,
        variant,
        move |event| print_event(&term_for_events, event, &plugin_id, variant_label),
    )?;

    let start_time = std::time::Instant::now();
    let result = engine.build()?;
    let elapsed = start_time.elapsed();

    // Matches Gradle's own final summary line and wording exactly
    // ("BUILD SUCCESSFUL in Xs" / "BUILD FAILED") — this is the line
    // most Gradle users' eyes jump straight to at the end of output.
    match result {
        Some(output) => {
            term.write_line(&format!(
                "\n{}\n{}",
                style(format!("BUILD SUCCESSFUL in {}s", elapsed.as_secs().max(1)))
                    .green()
                    .bold(),
                output.display()
            ))?;
            Ok(())
        }
        None => {
            term.write_line(&format!("\n{}", style("BUILD FAILED").red().bold()))?;
            anyhow::bail!("build failed")
        }
    }
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_lowercase();
    Ok(trimmed.is_empty() || trimmed == "y" || trimmed == "yes")
}

/// Maps this engine's internal stage names to Gradle-style task names,
/// e.g. "Kotlin/Java" -> "compileKotlin", the way a real Gradle build
/// prints ":app:compileDebugKotlin". The plugin's own pluginId stands
/// in for the module name Gradle would normally show (":app:", ":lib:")
/// since this tool has no multi-module concept — there's exactly one
/// module: the plugin being built.
fn gradle_style_task_name(stage: &str, variant_label: &str) -> String {
    // Stage names carrying a bracketed suffix (ABI-specific C++/Qt
    // stages, e.g. "Native (C++) [arm64-v8a]") keep that suffix
    // attached to the task name so multiple ABI runs remain
    // individually distinguishable in the output, the same way Gradle
    // itself would show separate tasks per build variant/flavor rather
    // than collapsing them into one line.
    let (base, suffix) = match stage.split_once(" [") {
        Some((base, rest)) => (base, format!("[{rest}")),
        None => (stage, String::new()),
    };

    let task = match base {
        "Resources" => format!("process{variant_label}Resources"),
        "Qt/QML" => format!("compile{variant_label}Qml"),
        "Native (C++)" => format!("compile{variant_label}Cpp"),
        "Native (Rust)" => format!("compile{variant_label}Rust"),
        "Kotlin/Java" => format!("compile{variant_label}Kotlin"),
        "Dexing" => format!("dex{variant_label}"),
        "Dexing (R8)" => format!("minify{variant_label}WithR8"),
        "Package & Align" => format!("package{variant_label}"),
        other => other.to_string(),
    };

    if suffix.is_empty() {
        task
    } else {
        format!("{task} {suffix}")
    }
}

fn print_event(term: &Term, event: BuildEvent, plugin_id: &str, variant_label: &str) {
    match event {
        // Gradle prints the task line the moment it starts, before any
        // of the task's own output streams in — not after. Matching
        // that ordering here means the task name must print now, with
        // its outcome (success/FAILED/UP-TO-DATE) appended once
        // StageFinished arrives — see below.
        BuildEvent::StageStarted { stage, .. } => {
            let task_name = gradle_style_task_name(&stage, variant_label);
            let _ = term.write_line(&format!(
                "{}",
                style(format!("> Task :{plugin_id}:{task_name}")).bold()
            ));
        }
        BuildEvent::StageOutput { line, severity, .. } => {
            let styled = match severity {
                LineSeverity::Error => style(line).red().to_string(),
                LineSeverity::Warning => style(line).yellow().to_string(),
                LineSeverity::Info => line,
            };
            let _ = term.write_line(&format!("  {styled}"));
        }
        BuildEvent::StageFinished {
            stage: _,
            success,
            warning_count,
            error_count,
            cached,
        } => {
            // Gradle appends the task's outcome marker inline with the
            // task line itself. This engine streams tool output as
            // separate lines in between (see StageOutput above), so an
            // identical single-line effect isn't possible in a plain
            // terminal stream — instead, print a short follow-up
            // marker only when it carries information beyond "it
            // succeeded with no remarks", which is exactly when Gradle
            // itself would show something (UP-TO-DATE, FAILED, or a
            // warning count) rather than nothing.
            if cached {
                let _ = term.write_line(&format!("  {}", style("UP-TO-DATE").cyan()));
            } else if !success {
                let _ = term.write_line(&format!(
                    "  {}",
                    style(format!("FAILED ({error_count} error(s))")).red().bold()
                ));
            } else if warning_count > 0 {
                let _ = term.write_line(&format!(
                    "  {}",
                    style(format!("({warning_count} warning(s))")).yellow()
                ));
            }
        }
        BuildEvent::BuildFinished { .. } => {
            // Final "BUILD SUCCESSFUL"/"BUILD FAILED" summary is
            // printed by the caller once `build()` returns, mirroring
            // Gradle's own final summary line rather than folding it
            // into per-task event handling.
        }
    }
}
