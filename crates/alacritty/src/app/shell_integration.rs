//! Per-session shell startup hooks. User configuration files are never modified.

use std::path::Path;
use std::{env, fs, io};

use alacritty_session::pty::Options;
use tempfile::TempDir;

pub fn prepare(options: &mut Options) -> io::Result<Option<TempDir>> {
    #[cfg(unix)]
    let default_shell = if options.shell.is_none() {
        alacritty_session::pty::default_shell_program()?
    } else {
        String::new()
    };
    #[cfg(windows)]
    let default_shell = String::new();
    let program = options.shell.as_ref().map_or(default_shell.as_str(), |shell| shell.program());
    if Path::new(program).file_name().is_none_or(|name| name != "zsh") {
        return Ok(None);
    }
    if options.shell.as_ref().is_some_and(|shell| {
        shell.args().iter().any(|arg| {
            !matches!(arg.as_str(), "-l" | "--login" | "-i" | "--interactive" | "-il" | "-li")
        })
    }) {
        return Ok(None);
    }

    let directory = tempfile::Builder::new().prefix("alacritty-shell-").tempdir()?;
    let script = directory.path().join("alacritty.zsh");
    fs::write(
        directory.path().join(".zshenv"),
        include_bytes!("../../../../assets/shell-integration/zsh/.zshenv"),
    )?;
    fs::write(&script, include_bytes!("../../../../assets/shell-integration/zsh/alacritty.zsh"))?;
    let original = options.env.get("ZDOTDIR").cloned().or_else(|| env::var("ZDOTDIR").ok());
    options
        .env
        .insert("ALACRITTY_ZDOTDIR_SET".into(), if original.is_some() { "1" } else { "0" }.into());
    options.env.insert("ALACRITTY_ZDOTDIR".into(), original.unwrap_or_default());
    options.env.insert("ALACRITTY_SHELL_INTEGRATION".into(), script.to_string_lossy().into_owned());
    options.env.insert("ZDOTDIR".into(), directory.path().to_string_lossy().into_owned());
    Ok(Some(directory))
}
