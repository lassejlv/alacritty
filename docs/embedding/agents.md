# Driving terminals from agents

This guide is for programs, including LLM agents, that run commands and
interactive applications through the Alacritty engine and read the results. It
uses the Python binding; every call maps to a C function in
[c-api.md](c-api.md). The helpers below live in
`crates/ffi/bindings/python/agent.py` and are tested against real programs by
`test_agent.py`.

```sh
make ffi                                   # or: cargo build --release -p alacritty_ffi
export ALACRITTY_FFI_LIBRARY=$PWD/target/release/alacritty_ffi/lib/libalacritty_ffi.dylib
export PYTHONPATH=$PWD/crates/ffi/bindings/python
```

Use `libalacritty_ffi.so` on Linux. The binding also finds the library on the
system library path or in this repository's `target/` directory.

## Why a terminal instead of pipes

Programs behave differently without a TTY: they buffer output, drop colors and
progress, refuse to prompt, or exit. A PTY-backed terminal gives the program the
environment it expects, and the engine turns the result into text, colors,
cursor position, command boundaries and exit codes. It also answers terminal
queries such as cursor position and colors, which otherwise make some programs
hang.

## Choose a pattern

| Task | Use |
| --- | --- |
| Run one command and collect its output | [`run`](#one-shot-commands) |
| Run several commands that share state (`cd`, variables, virtualenvs) | [`Shell`](#shell-sessions) |
| Operate a full-screen program (`less`, `vim`, `htop`, installers) | [Screen interaction](#full-screen-programs) |
| The program runs elsewhere (SSH, containers, web sockets) | [Headless terminals](#headless-terminals) |

## One-shot commands

`run` gives the program its own PTY, waits for it to exit and returns the exit
code with everything it printed, including scrollback:

```python
from agent import run

status, output = run(["cargo", "test"], timeout=600, cwd="/src/project")
```

The terminal is 200 columns wide by default so long lines don't wrap. Output is
kept up to the scrollback limit (10,000 lines); earlier lines are dropped. For
larger output, have the program write to a file. To pass a shell pipeline, run
`["/bin/sh", "-c", "a | b"]`.

## Shell sessions

`Shell` starts an interactive zsh with Alacritty's
[shell integration](../protocols/shell-integration.md), so the terminal knows
where each command's output starts and ends and what it returned. It doesn't
load the user's dotfiles, so the prompt and aliases are predictable.

```python
from agent import Shell

with Shell(cwd="/src/project") as shell:
    shell.run("source .venv/bin/activate")
    status, output = shell.run("pytest -q", timeout=300)
    print(shell.cwd)  # Reported by the shell after every command.
```

`run` types the command and Enter, then waits until the shell reports the
command finished (`OSC 133;D`) and shows the next prompt. The output is only
what the command printed, without prompts or the echoed command. Use
`run_script` for multi-line code: it is pasted with bracketed paste, so embedded
newlines don't submit partial lines.

Timeouts raise `TimeoutError` and leave the command running. Check
`shell.term.screen_text()` to see whether it is waiting for input, answer it with
`shell.term.type(...)`, or stop it:

```python
try:
    shell.run("npm install", timeout=120)
except TimeoutError:
    print(shell.term.screen_text())   # Inspect before deciding.
    shell.interrupt()                 # Ctrl+C; the exit status becomes 130.
```

Other shells work when they emit the same markers. The terminal reads these
sequences from any program:

| Sequence | Meaning |
| --- | --- |
| `ESC ] 133 ; A BEL` | Prompt starts |
| `ESC ] 133 ; B BEL` | Prompt ends, command input starts |
| `ESC ] 133 ; C BEL` | Command output starts |
| `ESC ] 133 ; D ; <status> BEL` | Command finished |

Without integration, fall back to `run` for each command, or wait for a known
prompt string with `term.wait_for(lambda t: t.screen_text().rstrip().endswith("$"))`.

## Full-screen programs

Full-screen programs redraw the screen instead of printing lines, so read the
screen rather than output:

```python
from agent import settle
from alacritty_ffi import MOD_CONTROL, MODE_ALT_SCREEN, Terminal

with Terminal.spawn(["less", "notes.txt"], columns=120, rows=40) as term:
    settle(term)                        # Wait until drawing stops.
    assert term.mode & MODE_ALT_SCREEN  # It switched to the alternate screen.
    term.type("/error\r")               # Search inside less.
    settle(term)
    print(term.screen_text())
    term.type("q")
    term.wait_for(lambda t: not t.running)
```

`settle` returns once no new output arrived for 0.3 seconds. Programs that
animate, such as spinners and clocks, never settle; wait for the text you expect
with `wait_for` instead.

Sending input:

- `type("text")` sends text as if typed. Use `\r` for Enter.
- `key("enter")`, `key("up")`, `key("page_down")`, `key("f5")` and
  `key("c", MOD_CONTROL)` send keys. They are encoded for whatever keyboard mode
  the program enabled, including application cursor keys and the Kitty keyboard
  protocol, so prefer `key` over hand-written escape sequences.
- `paste("text")` pastes, using bracketed paste when the program enabled it.
  Editors and shells then insert the text literally instead of running it.
- `write(b"...")` sends raw bytes and bypasses all encoding.

Reading the screen:

- `screen_text()` is the visible screen. `text_range(start, end)` reads any lines;
  0 is the top of the screen and negative lines are scrollback.
- `snapshot()` exposes each cell's character, colors and attributes. Use it when
  meaning is carried by style, like the selected item of a menu.
  `highlighted_rows(term)` returns rows drawn in reverse video or with a
  background color.
- `search(pattern)` finds a regex across the screen and scrollback and returns
  its position.
- `visible_links()` lists URLs and OSC 8 hyperlinks on screen.
- `title`, `working_directory()`, `shell_state()` and `mode` report what the
  program declared about itself.

## Headless terminals

When the program runs somewhere else, create a headless terminal and move bytes
yourself. Feed everything the remote side prints into `feed`; everything the
terminal wants to send back, including your input and its replies to queries,
arrives as `output` events:

```python
term = Terminal.headless(columns=120, rows=40)

def on_remote_data(data):
    term.feed(data)
    for event in term.events():
        if event.kind == "output":
            channel.send(event.data)

term.type("uname -a\r")   # Queued as an output event.
for event in term.events():
    if event.kind == "output":
        channel.send(event.data)
```

Forward every `output` event even if you didn't type anything: programs ask the
terminal for cursor positions and colors, and stall when the replies don't
arrive. Call `term.resize` when the remote window size changes, and tell the
remote side too (for SSH, a window-change request).

## Waiting correctly

- Use `wait_for(predicate, timeout)` for any condition. It sleeps until the
  terminal has new events instead of polling.
- Wait for state the program reports when possible: exit (`term.running`),
  command completion (`shell_state()`), an alternate screen switch (`mode`),
  or a title change. Text matching is the fallback.
- Always pass a timeout, and treat a timeout as a signal to inspect the screen.
  The program is often waiting for a confirmation you didn't expect.
- Call `events()` or `wait` regularly while a terminal is alive. Replies to
  programs and clipboard requests are handled there.

## Pitfalls

- **Wrapping.** Output wider than the terminal wraps, and `screen_text` joins
  wrapped lines back together. Narrow terminals make full-screen programs drop
  columns, so use at least 120 columns for agents.
- **Overwritten lines.** Progress bars rewrite one line with `\r`. The terminal
  keeps only the final state, which is usually what you want; use `run` with a
  log file when you need every line.
- **Interactive prompts.** Package managers and `git` may stop for input.
  Prefer non-interactive flags (`-y`, `--no-input`, `GIT_PAGER=cat`) or answer
  the prompt after a timeout.
- **Pagers.** `git log` and `man` open a pager in a terminal. Set
  `env={"PAGER": "cat", "GIT_PAGER": "cat"}` or use `--no-pager`.
- **Colors.** Text never contains escape sequences; styles are in snapshots. Set
  `NO_COLOR=1` only if you want programs to stop styling.
- **Secrets.** Everything a program prints stays in scrollback until the
  terminal is closed. Close terminals that displayed credentials.
- **Threads.** A terminal object belongs to one thread. To send input from
  another thread, use `term.writer()`.

## Other languages

The C API is the source of truth; the Python helpers are a thin layer over it.
The same patterns in C:

| Python | C |
| --- | --- |
| `Terminal.spawn` / `Terminal.headless` | `alac_terminal_spawn` / `alac_terminal_new_headless` |
| `wait`, `events` | wakeup callback, `alac_terminal_poll_event`, `alac_terminal_next_timeout_ms`, `alac_terminal_tick` |
| `type`, `key`, `paste` | `alac_terminal_text`, `alac_terminal_key`, `alac_terminal_paste` |
| `screen_text`, `text_range` | `alac_terminal_screen_text`, `alac_terminal_text_range` |
| `shell_state`, `last_command_output` | `alac_terminal_shell_state`, `alac_terminal_last_command_output` |
| `snapshot` | `alac_terminal_snapshot`, `alac_snapshot_info` |

Rust programs can use `alacritty_ffi::Engine` directly; see [README.md](README.md).
