use anyhow::Result;
use clap::Args;
use console::style;
use std::path::PathBuf;
use youki_toolchain::ToolchainPaths;

#[derive(Args)]
pub struct DoctorArgs {
    #[arg(long)]
    pub android_home: Option<PathBuf>,
    #[arg(long)]
    pub ndk_home: Option<PathBuf>,
}

pub fn run(args: DoctorArgs) -> Result<()> {
    let t = ToolchainPaths::discover(args.android_home.as_deref(), args.ndk_home.as_deref());

    println!("Youki Build toolchain report:\n");
    report("kotlinc", &t.kotlinc, "required for all plugin projects");
    report("android.jar", &t.android_jar, "required for all plugin projects");
    report("aapt2", &t.aapt2, "only needed if the project has a res/ directory");
    report("d8", &t.d8, "required for debug builds");
    report("r8", &t.r8, "required for release builds");
    report("zipalign", &t.zipalign, "required for the final packaging stage");
    report("clang++ (NDK)", &t.clangxx, "only needed if the project has plugin/cpp");
    report("cargo-ndk", &t.cargo_ndk, "only needed if the project has plugin/rust");
    report("cargo-clippy", &t.cargo_clippy, "only needed for --mode nightmare with Rust");
    report("cmake", &t.cmake, "only needed if the project has plugin/qml");

    Ok(())
}

fn report(name: &str, path: &Option<PathBuf>, note: &str) {
    match path {
        Some(p) => println!("  {} {name} -> {}", style("✓").green(), p.display()),
        None => println!("  {} {name} -> not found ({note})", style("✗").red()),
    }
}
