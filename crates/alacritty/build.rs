use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn main() {
    println!("cargo:rerun-if-env-changed=ALACRITTY_RELEASE_VERSION");
    let mut version = env::var("ALACRITTY_RELEASE_VERSION")
        .unwrap_or_else(|_| String::from(env!("CARGO_PKG_VERSION")));
    if let Some(commit_hash) = commit_hash() {
        version = format!("{version} ({commit_hash})");
    }
    println!("cargo:rustc-env=VERSION={version}");

    embed_themes();

    #[cfg(windows)]
    embed_resource::compile("../../packaging/windows/alacritty.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
}

/// Embed every `themes/*.toml` file as a built-in theme named after its file stem.
fn embed_themes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("themes");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut themes: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("read themes directory")
        .map(|entry| entry.expect("read themes directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "toml"))
        .collect();
    themes.sort();

    let mut source = String::from("pub static BUILTIN_THEMES: &[(&str, &str)] = &[\n");
    for path in themes {
        let name = path.file_stem().unwrap().to_str().expect("UTF-8 theme name");
        writeln!(source, "    ({name:?}, include_str!({:?})),", path.display().to_string())
            .unwrap();
    }
    source.push_str("];\n");

    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("themes.rs");
    fs::write(out, source).expect("write embedded themes");
}

fn commit_hash() -> Option<String> {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|hash| hash.trim().into())
}
