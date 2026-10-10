//! Compares fresh snapshots with incremental snapshot updates.
//!
//! Run with `cargo run --release -p alacritty_ffi --example snapshot_bench`.

use std::hint::black_box;
use std::time::{Duration, Instant};

use alacritty_ffi::snapshot::Snapshot;
use alacritty_ffi::{Engine, Options};

const COLUMNS: usize = 200;
const ROWS: usize = 60;
const FRAMES: u32 = 2_000;

fn engine() -> Engine {
    let options = Options { columns: COLUMNS, rows: ROWS, ..Options::default() };
    let mut engine = Engine::headless(options, None);
    for row in 0..ROWS {
        let line: String =
            (0..COLUMNS).map(|column| (b'a' + ((row + column) % 26) as u8) as char).collect();
        let color = 31 + row % 7;
        engine.feed(format!("\x1b[{color}m{line}\x1b[0m").as_bytes());
        if row + 1 < ROWS {
            engine.feed(b"\r\n");
        }
    }
    engine.feed(b"\x1b[30;1H");
    engine
}

/// Time `FRAMES` frames, each changing the terminal with `change` and capturing it with
/// `capture`.
fn measure(
    change: impl Fn(&mut Engine, u32),
    mut capture: impl FnMut(&mut Engine, &mut Snapshot),
) -> Duration {
    let mut engine = engine();
    let mut snapshot = Snapshot::default();
    capture(&mut engine, &mut snapshot);
    let start = Instant::now();
    for frame in 0..FRAMES {
        change(&mut engine, frame);
        capture(&mut engine, &mut snapshot);
        black_box(&snapshot);
    }
    start.elapsed() / FRAMES
}

fn main() {
    let typing = |engine: &mut Engine, frame: u32| {
        // Type across one line, returning to its start when full.
        let byte =
            if frame.is_multiple_of(COLUMNS as u32) { b'\r' } else { b'a' + (frame % 26) as u8 };
        engine.feed(&[byte]);
    };
    let scrolling = |engine: &mut Engine, frame: u32| {
        engine.feed(format!("\x1b[60;1H\r\noutput line {frame}").as_bytes());
    };

    println!("{COLUMNS}x{ROWS} grid, mean per frame over {FRAMES} frames");
    for (name, change) in
        [("typing", &typing as &dyn Fn(&mut Engine, u32)), ("scrolling", &scrolling)]
    {
        let fresh = measure(change, |engine, snapshot| *snapshot = engine.snapshot());
        let incremental = measure(change, |engine, snapshot| engine.update_snapshot(snapshot));
        println!(
            "{name:>10}: fresh {:>8.1?}  incremental {:>8.1?}  ({:.1}x)",
            fresh,
            incremental,
            fresh.as_secs_f64() / incremental.as_secs_f64()
        );
    }
}
