use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=ALACRITTY_RELEASE_VERSION");
    let mut version = env::var("ALACRITTY_RELEASE_VERSION")
        .unwrap_or_else(|_| String::from(env!("CARGO_PKG_VERSION")));
    if let Some(commit_hash) = commit_hash() {
        version = format!("{version} ({commit_hash})");
    }
    println!("cargo:rustc-env=VERSION={version}");

    #[cfg(windows)]
    embed_resource::compile("../../packaging/windows/alacritty.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
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
