//! Runs one external tool and streams its output live, exactly the
//! way `PluginBuildEngine.kt::runTool` did: byte-by-byte, flushing on
//! both `\n` and `\r`, so tools like `cargo`/`rustc` that redraw
//! progress with carriage returns look "live" instead of frozen.
//!
//! Rust's `std::process` gives us pipes as `Read`, so instead of
//! manually decoding one byte at a time in a loop (fine in Kotlin, but
//! not idiomatic here) we use a small buffered reader that flushes on
//! either terminator, preserving the same UX guarantee with idiomatic
//! Rust I/O.

use crate::types::{classify, BuildEvent, LineSeverity};
use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct ToolRun<'a> {
    pub stage_name: &'a str,
    pub program: &'a Path,
    pub args: &'a [String],
    pub working_dir: &'a Path,
}

/// Runs the tool, forwarding every classified line to `on_event`, and
/// returns whether the process exited successfully (exit code 0).
///
/// `warnings`/`errors` are accumulated into the counters the caller
/// passes in, matching the original's mutable `totalWarnings`/
/// `totalErrors` fields on the engine instance.
pub fn run_tool(
    run: ToolRun,
    warnings: &mut u32,
    errors: &mut u32,
    mut on_event: impl FnMut(BuildEvent),
) -> Result<bool> {
    let cmd_display = format!(
        "$ {} {}",
        run.program.display(),
        run.args.join(" ")
    );
    emit(run.stage_name, &cmd_display, warnings, errors, &mut on_event);

    let mut child = Command::new(run.program)
        .args(run.args)
        .current_dir(run.working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning {}", run.program.display()))?;

    // Merge stdout+stderr into one stream, same as the original's
    // redirectErrorStream(true), by reading both from separate threads
    // into a shared channel preserving arrival order isn't guaranteed
    // across the two OS pipes — but neither was it in the Kotlin
    // version once redirectErrorStream merges them at the OS level.
    // The simplest faithful approach here: read stdout and stderr
    // sequentially isn't correct either (would block). Use a real
    // merge via os_pipe-free approach: spawn with stderr piped to the
    // same fd isn't directly expressible in std, so we read both
    // concurrently with threads and forward as they arrive.
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let (tx, rx) = std::sync::mpsc::channel::<String>();

    let tx_out = tx.clone();
    let out_handle = std::thread::spawn(move || stream_lines(stdout, tx_out));
    let err_handle = std::thread::spawn(move || stream_lines(stderr, tx));

    for line in rx {
        emit(run.stage_name, &line, warnings, errors, &mut on_event);
    }

    let _ = out_handle.join();
    let _ = err_handle.join();

    let status = child.wait().context("waiting for process")?;
    Ok(status.success())
}

/// Reads a stream byte-by-byte, flushing the accumulated buffer as a
/// line whenever '\n' or '\r' is seen — the same terminator-agnostic
/// splitting the Kotlin version used so that `\r`-redrawn progress
/// lines (cargo, rustc) still surface promptly instead of buffering
/// until a real newline.
fn stream_lines(mut reader: impl Read, tx: std::sync::mpsc::Sender<String>) {
    let mut buffer = String::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                let ch = byte[0] as char;
                if ch == '\n' || ch == '\r' {
                    if !buffer.is_empty() {
                        let _ = tx.send(std::mem::take(&mut buffer));
                    }
                } else {
                    buffer.push(ch);
                }
            }
            Err(_) => break,
        }
    }
    if !buffer.is_empty() {
        let _ = tx.send(buffer);
    }
}

fn emit(
    stage: &str,
    line: &str,
    warnings: &mut u32,
    errors: &mut u32,
    on_event: &mut impl FnMut(BuildEvent),
) {
    let severity = classify(line);
    match severity {
        LineSeverity::Warning => *warnings += 1,
        LineSeverity::Error => *errors += 1,
        LineSeverity::Info => {}
    }
    on_event(BuildEvent::StageOutput {
        stage: stage.to_string(),
        line: line.to_string(),
        severity,
    });
}
