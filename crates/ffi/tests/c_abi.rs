//! Checks that `include/alacritty.h` matches the Rust ABI and that the C examples run.

#![cfg(unix)]

use std::fmt::Write as _;
use std::mem::{offset_of, size_of};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use alacritty_ffi::ffi::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

macro_rules! layout {
    ($source:expr, $ty:ident { $($field:ident),* $(,)? }) => {{
        writeln!($source, "_Static_assert(sizeof({}) == {}, \"{0} size\");", stringify!($ty), size_of::<$ty>()).unwrap();
        $(
            writeln!(
                $source,
                "_Static_assert(offsetof({}, {}) == {}, \"{0}.{1}\");",
                stringify!($ty),
                stringify!($field),
                offset_of!($ty, $field),
            )
            .unwrap();
        )*
    }};
}

macro_rules! constants {
    ($source:expr, $($name:ident),* $(,)?) => {
        $(
            writeln!($source, "_Static_assert({} == {}u, \"{0}\");", stringify!($name), $name).unwrap();
        )*
    };
}

/// Directory containing the library artifacts built for this test.
///
/// The shared library is also installed under its versioned soname or install name, which
/// linked programs load at runtime.
fn library_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
        let (library, versioned) = if cfg!(target_os = "macos") {
            ("libalacritty_ffi.dylib", format!("libalacritty_ffi.{ALAC_ABI_VERSION}.dylib"))
        } else {
            ("libalacritty_ffi.so", format!("libalacritty_ffi.so.{ALAC_ABI_VERSION}"))
        };
        // Test binaries run concurrently, so copy to a unique name and move it in place.
        let temporary = dir.join(format!("{versioned}.{}", std::process::id()));
        std::fs::copy(dir.join(library), &temporary).unwrap();
        std::fs::rename(&temporary, dir.join(versioned)).unwrap();
        dir
    })
    .clone()
}

fn compiler() -> String {
    std::env::var("CC").unwrap_or_else(|_| "cc".into())
}

fn compile(source: &Path, output: &Path, extra: &[String]) {
    let library_dir = library_dir();
    let status = Command::new(compiler())
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Werror")
        .arg("-I")
        .arg(Path::new(ROOT).join("include"))
        .arg(source)
        .arg("-L")
        .arg(&library_dir)
        .arg("-lalacritty_ffi")
        .arg(format!("-Wl,-rpath,{}", library_dir.display()))
        .args(extra)
        .arg("-o")
        .arg(output)
        .status()
        .expect("C compiler");
    assert!(status.success(), "failed to compile {}", source.display());
}

#[test]
fn header_layout_matches_rust() {
    let mut source = String::from("#include <stddef.h>\n#include \"alacritty.h\"\n");
    layout!(source, AlacOptions {
        columns,
        rows,
        cell_width,
        cell_height,
        scrollback,
        osc52,
        cursor_shape,
        cursor_blinking,
        kitty_keyboard,
        bold_is_bright,
        scroll_on_input,
        desktop_notifications,
        semantic_escape_chars,
    });
    layout!(source, AlacPtyOptions {
        program,
        args,
        arg_count,
        working_directory,
        env,
        env_count,
        drain_on_exit,
    });
    layout!(source, AlacKeyEvent {
        action,
        key,
        codepoint,
        unshifted_codepoint,
        text,
        location,
        mods,
        alt_composes,
    });
    layout!(source, AlacProgress { state, percent });
    layout!(source, AlacNotification {
        serial,
        id,
        title,
        body,
        sound,
        application,
        buttons,
        button_count,
        icons,
        icon_count,
        urgency,
        occasion,
        expires_in_ms,
        focus,
        report,
        report_close,
        persistent,
    });
    layout!(source, AlacEvent { kind, exit_code, serial, data, len, progress, notification });
    layout!(source, AlacClipboardItem { mime_type, data, len });
    layout!(source, AlacClipboardCallbacks { userdata, formats, read, write });
    layout!(source, AlacCell {
        codepoint,
        fg,
        bg,
        underline_color,
        flags,
        zerowidth_index,
        zerowidth_count,
        reserved,
        hyperlink,
    });
    layout!(source, AlacCursor { column, row, shape, color, text_color, blinking, wide });
    layout!(source, AlacLineDamage { row, left, right });
    layout!(source, AlacImage {
        image_id,
        placement_id,
        generation,
        width,
        height,
        rgba,
        rgba_len,
        png,
        png_len,
        z_index,
        dest_x,
        dest_y,
        dest_width,
        dest_height,
        src_x,
        src_y,
        src_width,
        src_height,
    });
    layout!(source, AlacSnapshotInfo {
        columns,
        rows,
        display_offset,
        history_size,
        mode,
        foreground,
        background,
        full_damage,
        cursor,
        cells,
        codepoints,
        codepoint_count,
        damage,
        damage_count,
        images,
        image_count,
    });
    layout!(source, AlacMatch { start_line, start_column, end_line, end_column });
    layout!(source, AlacShellState { phase, has_last_command, last_command_id, last_exit_status });
    layout!(source, AlacProgramStatus { id, app, title, message, state, blocked_kind, progress });
    layout!(source, AlacLink { uri, range, hyperlink });

    constants!(
        source,
        ALAC_ABI_VERSION,
        ALAC_OSC52_DISABLED,
        ALAC_OSC52_COPY_PASTE,
        ALAC_KEY_RELEASE,
        ALAC_LOCATION_NUMPAD,
        ALAC_KEY_F1,
        ALAC_MOUSE_RIGHT,
        ALAC_WHEEL_SCROLLED,
        ALAC_SCROLL_BOTTOM,
        ALAC_EVENT_WAKEUP,
        ALAC_EVENT_TITLE,
        ALAC_EVENT_BELL,
        ALAC_EVENT_CURSOR_BLINKING_CHANGE,
        ALAC_EVENT_MOUSE_CURSOR_DIRTY,
        ALAC_EVENT_PROGRAM_STATUS_CHANGED,
        ALAC_EVENT_PROGRESS_CHANGED,
        ALAC_EVENT_NOTIFICATION_SHOW,
        ALAC_EVENT_NOTIFICATION_CLOSE,
        ALAC_EVENT_NOTIFICATION_QUERY_ALIVE,
        ALAC_EVENT_OUTPUT,
        ALAC_EVENT_CHILD_EXIT,
        ALAC_EVENT_EXIT,
        ALAC_EVENT_WORKING_DIRECTORY_CHANGED,
        ALAC_COLOR_COUNT,
        ALAC_CLIPBOARD_PRIMARY,
        ALAC_SELECTION_LINES,
        ALAC_SIDE_RIGHT,
        ALAC_SHELL_OUTPUT,
        ALAC_CELL_SELECTED,
        ALAC_CELL_DEFAULT_BACKGROUND,
        ALAC_CELL_UNDERLINE_COLOR,
        ALAC_CELL_IMAGE_PLACEHOLDER,
        ALAC_VI_UP,
        ALAC_VI_FIRST_OCCUPIED,
        ALAC_VI_SEMANTIC_RIGHT_END,
        ALAC_VI_WORD_RIGHT_END,
        ALAC_VI_PARAGRAPH_DOWN,
    );

    // Raw flags and modes are shared with the terminal crate.
    use alacritty_ffi::alacritty_terminal::term::TermMode;
    use alacritty_ffi::alacritty_terminal::term::cell::Flags;
    for (name, value) in [
        ("ALAC_CELL_INVERSE", Flags::INVERSE.bits()),
        ("ALAC_CELL_DIM", Flags::DIM.bits()),
        ("ALAC_CELL_DASHED_UNDERLINE", Flags::DASHED_UNDERLINE.bits()),
    ] {
        writeln!(source, "_Static_assert({name} == {value}u, \"{name}\");").unwrap();
    }
    for (name, value) in [
        ("ALAC_MODE_APP_CURSOR", TermMode::APP_CURSOR.bits()),
        ("ALAC_MODE_ALT_SCREEN", TermMode::ALT_SCREEN.bits()),
        ("ALAC_MODE_VI", TermMode::VI.bits()),
        ("ALAC_MODE_REPORT_ASSOCIATED_TEXT", TermMode::REPORT_ASSOCIATED_TEXT.bits()),
        ("ALAC_MODE_CLIPBOARD_PASTE_EVENTS", TermMode::CLIPBOARD_PASTE_EVENTS.bits()),
    ] {
        writeln!(source, "_Static_assert({name} == {value}u, \"{name}\");").unwrap();
    }

    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("alacritty_ffi_layout");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("layout.c");
    std::fs::write(&path, source).unwrap();
    let status = Command::new(compiler())
        .args(["-std=c11", "-Wall", "-Werror", "-fsyntax-only", "-I"])
        .arg(Path::new(ROOT).join("include"))
        .arg(&path)
        .status()
        .expect("C compiler");
    assert!(status.success(), "header layout differs from Rust");
}

fn run_example(name: &str, args: &[&str]) -> String {
    run_example_with(name, args, &[], &[])
}

fn run_example_with(name: &str, args: &[&str], flags: &[String], env: &[(&str, &str)]) -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("alacritty_ffi_examples");
    std::fs::create_dir_all(&dir).unwrap();
    let binary = dir.join(name);
    compile(&Path::new(ROOT).join("examples").join(format!("{name}.c")), &binary, flags);
    let output = Command::new(&binary).args(args).envs(env.iter().copied()).output().unwrap();
    check_output(name, output)
}

fn check_output(name: &str, output: std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{name} failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

#[test]
fn headless_example_runs() {
    let stdout = run_example("headless", &[]);
    assert!(stdout.contains("notification: done"), "{stdout}");
    assert!(stdout.ends_with("ok\n"), "{stdout}");
}

#[test]
fn pty_example_runs() {
    let stdout = run_example("pty", &["printf 'agent says hi'"]);
    assert!(stdout.contains("agent says hi"), "{stdout}");
    assert!(stdout.contains("[exit 0]"), "{stdout}");
}

/// Compiler flags for a pkg-config package, or `None` when it isn't installed.
fn pkg_config(packages: &[&str]) -> Option<Vec<String>> {
    let output =
        Command::new("pkg-config").arg("--cflags").arg("--libs").args(packages).output().ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout).split_whitespace().map(str::to_owned).collect()
    })
}

#[test]
fn sdl_example_runs() {
    let Some(flags) = pkg_config(&["sdl2", "SDL2_ttf"]) else {
        eprintln!("skipping: sdl2 and SDL2_ttf not found by pkg-config");
        return;
    };
    let font = std::env::var("ALACRITTY_FONT").unwrap_or_else(|_| {
        let font = if cfg!(target_os = "macos") {
            "/System/Library/Fonts/Menlo.ttc"
        } else {
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"
        };
        font.into()
    });
    if !Path::new(&font).exists() {
        eprintln!("skipping: font {font} not found");
        return;
    }
    let args = ["--font", &font, "--exit-after", "5000", "--command", "printf 'sdl host'"];
    let env = [("SDL_VIDEODRIVER", "dummy")];
    let stdout = run_example_with("sdl_host", &args, &flags, &env);
    assert!(stdout.contains("sdl host"), "{stdout}");
    assert!(stdout.contains("frames: "), "{stdout}");
}

#[test]
fn python_binding_runs() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipping: python3 not found");
        return;
    }
    let library = library_dir().join(if cfg!(target_os = "macos") {
        "libalacritty_ffi.dylib"
    } else {
        "libalacritty_ffi.so"
    });
    let bindings = Path::new(ROOT).join("bindings/python");
    for script in ["check_header.py", "example.py", "test_agent.py"] {
        let output = Command::new("python3")
            .arg(bindings.join(script))
            .env("ALACRITTY_FFI_LIBRARY", &library)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .unwrap();
        let stdout = check_output(script, output);
        if script != "check_header.py" {
            assert!(stdout.ends_with("ok\n"), "{stdout}");
        }
    }
}
