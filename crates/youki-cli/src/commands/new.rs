use anyhow::{Context, Result};
use clap::Args;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Args)]
pub struct NewArgs {
    /// Plugin project name, also used as the directory name
    pub name: String,

    /// Java-style package id, e.g. com.example.myplugin
    #[arg(long)]
    pub package: Option<String>,
}

pub fn run(args: NewArgs) -> Result<()> {
    // SECURITY: `name` becomes a directory created directly under the
    // current working directory. Passing it straight to
    // `PathBuf::from` let a name like "../../some/other/place" escape
    // the current directory entirely — confirmed in testing: this
    // really did create a project outside cwd. A project *name* has
    // no legitimate reason to contain a path separator or a parent-dir
    // reference at all, so this rejects the input outright rather than
    // silently reinterpreting it (unlike plugin.json's dexPath, where
    // sanitizing-and-continuing made sense because a build shouldn't
    // abort over a manifest quirk — here, the intent is genuinely
    // ambiguous enough that guessing what the user meant is the wrong
    // call).
    anyhow::ensure!(
        !args.name.is_empty(),
        "project name cannot be empty"
    );
    let has_only_one_normal_component = {
        let mut components = Path::new(&args.name).components();
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
    };
    anyhow::ensure!(
        has_only_one_normal_component,
        "project name must be a plain directory name with no path separators (got: {:?})",
        args.name
    );

    let package = args
        .package
        .unwrap_or_else(|| format!("com.example.{}", args.name.to_lowercase()));

    let root = PathBuf::from(&args.name);
    anyhow::ensure!(!root.exists(), "directory {} already exists", root.display());

    // New scaffolds always use plugin/kotlin/ — pure-Kotlin sources.
    // A project that mixes in Java should rename this directory to
    // plugin/java/ manually (kotlinc compiles both extensions found
    // there together); youki new doesn't guess that need up front.
    let src_dir = root.join("plugin/kotlin").join(package.replace('.', "/"));
    fs::create_dir_all(&src_dir).context("creating source directory")?;

    fs::write(
        root.join("plugin.json"),
        format!(
            r#"{{
  "pluginId": "{package}",
  "displayName": "{name}",
  "version": "1.0.0",
  "enabled": false,
  "permissions": [],
  "surface": "CARD_GRID",
  "entry": {{
    "type": "native_fragment",
    "dexPath": "classes.dex",
    "mainClass": "{package}.MainEntry"
  }}
}}
"#,
            name = args.name
        ),
    )?;

    fs::write(
        src_dir.join("MainEntry.kt"),
        format!(
            r#"package {package}

class MainEntry {{
    fun onLoad() {{
        // Plugin entry point.
    }}
}}
"#
        ),
    )?;

    fs::write(root.join(".gitignore"), "build/\noutput/\n")?;

    println!("Created new plugin project at {}", root.display());
    println!("Next steps:");
    println!("  cd {}", args.name);
    println!("  youki build");

    Ok(())
}
