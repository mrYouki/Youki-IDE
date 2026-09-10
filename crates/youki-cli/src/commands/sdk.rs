use anyhow::Result;
use clap::Subcommand;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use youki_sdk_manager::SdkManager;

#[derive(Subcommand)]
pub enum SdkAction {
    /// List installed SDK/NDK components
    List {
        #[arg(long)]
        android_home: Option<PathBuf>,
    },
    /// Install a platform (android.jar) for the given API level
    InstallPlatform {
        api_level: u32,
        #[arg(long)]
        android_home: Option<PathBuf>,
    },
    /// Install build-tools (aapt2, d8, r8, zipalign)
    InstallBuildTools {
        version: String,
        #[arg(long)]
        android_home: Option<PathBuf>,
    },
    /// Install the Android NDK (needed for C++/Rust plugin stages)
    InstallNdk {
        version: String,
        #[arg(long)]
        android_home: Option<PathBuf>,
    },
}

fn resolve_home(override_path: Option<PathBuf>) -> PathBuf {
    override_path.unwrap_or_else(SdkManager::default_location)
}

pub fn run(action: SdkAction) -> Result<()> {
    match action {
        SdkAction::List { android_home } => {
            let mgr = SdkManager::new(resolve_home(android_home));
            let installed = mgr.list_installed();
            if installed.is_empty() {
                println!("No SDK components installed yet at {}", mgr.android_home.display());
            } else {
                println!("Installed at {}:", mgr.android_home.display());
                for c in installed {
                    println!("  {} ({})", style(&c.id).bold(), c.path.display());
                }
            }
            Ok(())
        }
        SdkAction::InstallPlatform { api_level, android_home } => {
            let mgr = SdkManager::new(resolve_home(android_home));
            if mgr.is_platform_installed(api_level) {
                println!("platform-{api_level} already installed");
                return Ok(());
            }
            let bar = progress_bar(&format!("Downloading platform-{api_level}"));
            mgr.install_platform(api_level, |done, total| update_bar(&bar, done, total))?;
            bar.finish_with_message("done");
            Ok(())
        }
        SdkAction::InstallBuildTools { version, android_home } => {
            let mgr = SdkManager::new(resolve_home(android_home));
            if mgr.is_build_tools_installed(&version) {
                println!("build-tools {version} already installed");
                return Ok(());
            }
            let bar = progress_bar(&format!("Downloading build-tools {version}"));
            mgr.install_build_tools(&version, |done, total| update_bar(&bar, done, total))?;
            bar.finish_with_message("done");
            Ok(())
        }
        SdkAction::InstallNdk { version, android_home } => {
            let mgr = SdkManager::new(resolve_home(android_home));
            if mgr.is_ndk_installed(&version) {
                println!("NDK {version} already installed");
                return Ok(());
            }
            let bar = progress_bar(&format!("Downloading NDK {version} (this is large, please wait)"));
            mgr.install_ndk(&version, |done, total| update_bar(&bar, done, total))?;
            bar.finish_with_message("done");
            Ok(())
        }
    }
}

fn progress_bar(message: &str) -> ProgressBar {
    let bar = ProgressBar::new(0);
    bar.set_style(
        ProgressStyle::with_template("{msg} [{bar:40.cyan/blue}] {bytes}/{total_bytes}")
            .unwrap()
            .progress_chars("=> "),
    );
    bar.set_message(message.to_string());
    bar
}

fn update_bar(bar: &ProgressBar, done: u64, total: u64) {
    if total > 0 && bar.length() != Some(total) {
        bar.set_length(total);
    }
    bar.set_position(done);
}
