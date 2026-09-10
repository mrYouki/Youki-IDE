//! Shared types for the build pipeline. Mirrors the sealed classes and
//! enums at the top of the original `PluginBuildEngine.kt`.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMode {
    Peaceful,
    Dictator,
    Nightmare,
}

/// Debug vs Release — independent of [BuildMode]. BuildMode controls
/// strictness (which warnings become errors); BuildVariant controls
/// what the DEXing stage does with the compiled classes: DEBUG uses d8
/// only (fast, no shrinking); RELEASE uses r8 instead of d8 (shrinks +
/// obfuscates, still produces the final classes.dex).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildVariant {
    Debug,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineSeverity {
    Info,
    Warning,
    Error,
}

/// Classifies a line of tool output the same way the Kotlin engine
/// did — purely additive metadata for UI coloring; the raw text is
/// never altered or dropped.
pub fn classify(line: &str) -> LineSeverity {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("error:")
        || trimmed.contains(": error:")
        || trimmed.starts_with("error[")
        || trimmed.starts_with("FAILURE:")
    {
        LineSeverity::Error
    } else if lower.starts_with("warning:") || trimmed.contains(": warning:") {
        LineSeverity::Warning
    } else {
        LineSeverity::Info
    }
}

#[derive(Debug, Clone)]
pub enum BuildEvent {
    StageStarted {
        stage: String,
        index: usize,
        total: usize,
    },
    StageOutput {
        stage: String,
        line: String,
        severity: LineSeverity,
    },
    StageFinished {
        stage: String,
        success: bool,
        warning_count: u32,
        error_count: u32,
        /// True if the stage was skipped because the build cache
        /// fingerprint matched and its outputs were already present —
        /// the tool never actually ran.
        cached: bool,
    },
    BuildFinished {
        success: bool,
        output_file: Option<PathBuf>,
        total_warnings: u32,
        total_errors: u32,
    },
}
